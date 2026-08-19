//! Edit-Track-Information dialog callbacks.
//!
//! The `TagEditor` global (`melodia-ui/ui/globals/dialog-forms.slint`) is Rust-owned: `request-edit`
//! fetches the selected rows and fills the fields + cover, `pick-artwork` /
//! `remove-artwork` drive the cover panel, and `commit` (fired by the
//! `Dialog.accepted` dispatcher) diffs the form against the populate-time
//! snapshot into a [`TagEdit`] and hands it to [`library::tags::apply_tag_edit`].
//!
//! Numbers cross the Slint boundary as strings, so all parse + validation lives
//! here; only touched fields become a `Set` / `Clear`.
//!
//! **All three async handlers use `slint::spawn_local` (not `runtime.spawn`)**:
//! the per-open snapshot lives in an `Rc<RefCell<_>>` and the completion toast
//! needs the `Rc<NotificationsUi>` — both `!Send`, so the work must stay on the
//! UI thread. `async_compat::Compat` supplies the tokio reactor for the awaited
//! sqlx / `spawn_blocking` calls, exactly as `ui::playlists::wire_files` does. Wired
//! from `main.rs` after the notifications stack exists, for the same reason.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, Model, Rgb8Pixel, SharedPixelBuffer, SharedString};

use crate::entities::track::TagEditRow;
use crate::error::AppError;
use crate::library;
use crate::library::tags::TagEditReport;
use crate::media::image_decode::{
    FilterType, MAX_SOURCE_DIM, decode_capped, fit_within, resize_rgb8,
};
use crate::media::tag_writer::{self, ArtworkEdit, FieldEdit, TagEdit};
use crate::state::AppState;
use crate::ui::file_dialog;
use crate::ui::shell::notifications::{NotificationParams, NotificationsUi};
use crate::ui::util::{COVER_SIZE, buffer_from_rgb};
use crate::{AppWindow, Dialog, Settings, TagEditor};

/// Canonical field order, shared by the three positional lists that must stay
/// aligned: the `commit` getter array, `populate`'s `field!` calls, and
/// [`build_edit`] / `TagSession::originals`. Indexing by name (not raw `0..12`)
/// makes the alignment explicit — reorder here and in all three sites together.
const TITLE: usize = 0;
const ARTIST: usize = 1;
const ALBUM_ARTIST: usize = 2;
const ALBUM: usize = 3;
const GENRE: usize = 4;
const YEAR: usize = 5;
const ORIGINAL_YEAR: usize = 6;
const TRACK_NUMBER: usize = 7;
const DISC_NUMBER: usize = 8;
const COMPOSER: usize = 9;
const COMMENT: usize = 10;
const BPM: usize = 11;
const LYRICS: usize = 12;

/// Number of editable fields (`title`..`lyrics`).
const FIELD_COUNT: usize = LYRICS + 1;

/// Auto-dismiss window for the completion toast, matching the playlist
/// import/export toasts.
const TOAST_MS: u32 = 3000;

/// Snapshot of one dialog open, shared across the four UI-thread handlers via
/// `Rc<RefCell<_>>` (the `sleep_timer` pattern). `request-edit` overwrites the
/// whole thing, so nothing leaks across opens.
#[derive(Default)]
struct TagSession {
    ids: Vec<i64>,
    /// Each editable field's value **as populated into the Slint property**, so
    /// the commit diff is a plain string compare.
    originals: Vec<String>,
    artwork: ArtworkEdit,
    /// The picked cover path — set only while `artwork == Replace`. Rides to the
    /// orchestrator as `apply_tag_edit`'s separate `artwork_source` arg.
    picked: Option<PathBuf>,
}

/// Wire the four `TagEditor` callbacks. Needs `Rc<NotificationsUi>` for the
/// Save completion toast, so it is called from `main.rs` after the notifications
/// stack exists (same constraint as `ui::playlists::wire_files`).
pub fn wire_tags(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let session: Rc<RefCell<TagSession>> = Rc::new(RefCell::new(TagSession::default()));
    let te = ui.global::<TagEditor>();

    wire_request_edit(&te, ui, state, &session);
    wire_pick_artwork(&te, ui, state, &session);
    wire_remove_artwork(&te, ui, &session);
    wire_commit(&te, ui, state, &session, notifications);
}

/// `request-edit`: fetch rows (+ lyrics + cover for a single selection),
/// populate, and open the dialog on the next UI tick.
fn wire_request_edit(
    te: &TagEditor,
    ui: &AppWindow,
    state: &AppState,
    session: &Rc<RefCell<TagSession>>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    let session = session.clone();
    te.on_request_edit(move |ids_model| {
        let ids: Vec<i64> = ids_model.iter().map(i64::from).collect();
        if ids.is_empty() {
            return;
        }
        let weak = weak.clone();
        let s = state.clone();
        let session = session.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            let rows = match library::tags::get_tag_edit_rows(&s, &ids).await {
                Ok(rows) if !rows.is_empty() => rows,
                Ok(_) => return,
                Err(e) => {
                    log::warn!("tag edit fetch: {e}");
                    return;
                }
            };
            let single = rows.len() == 1;

            // Lyrics are stored in the file, not the DB — read them for a
            // single selection (the Lyrics tab isn't mounted otherwise).
            let lyrics = if single {
                let path = PathBuf::from(&rows[0].file_path);
                match s.runtime.spawn_blocking(move || tag_writer::read_lyrics(&path)).await {
                    Ok(Ok(Some(l))) => l,
                    _ => String::new(),
                }
            } else {
                String::new()
            };

            // Cover preview from the first row that has one (decode off the
            // UI thread — a full-res source could jank it).
            let cover_path = rows.iter().find_map(|r| {
                r.artwork_path.as_deref().filter(|p| !p.is_empty()).map(PathBuf::from)
            });
            let cover = match cover_path {
                Some(p) => {
                    s.runtime.spawn_blocking(move || decode_cover_preview(&p)).await.ok().flatten()
                }
                None => None,
            };

            let Some(ui) = weak.upgrade() else { return };
            populate(&ui, &session, &rows, lyrics, cover);
            ui.global::<Dialog>().set_open(true);
        }));
    });
}

/// `pick-artwork`: native image picker → decode preview → stash as a Replace.
fn wire_pick_artwork(
    te: &TagEditor,
    ui: &AppWindow,
    state: &AppState,
    session: &Rc<RefCell<TagSession>>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    let session = session.clone();
    te.on_pick_artwork(move || {
        let weak = weak.clone();
        let s = state.clone();
        let session = session.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            // Filter broadly — the orchestrator normalizes on write (lofty's
            // accepted set and MP4's differ; no single filter expresses it).
            let dialog = file_dialog::parented(&weak, "Choose Cover Image")
                .add_filter("Images", &["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"]);
            let Some(handle) = dialog.pick_file().await else {
                return;
            };
            let path = handle.path().to_path_buf();
            let decode_path = path.clone();
            let buf = s
                .runtime
                .spawn_blocking(move || decode_cover_preview(&decode_path))
                .await
                .ok()
                .flatten();

            let Some(ui) = weak.upgrade() else { return };
            let te = ui.global::<TagEditor>();
            if let Some(buf) = buf {
                te.set_cover(Image::from_rgb8(buf));
                te.set_has_cover(true);
                let mut sess = session.borrow_mut();
                sess.artwork = ArtworkEdit::Replace;
                sess.picked = Some(path);
            } else {
                // Preview-only failure — leave the session untouched, so
                // Save won't try to embed an image it couldn't even decode.
                log::warn!("cover preview decode failed: {}", path.display());
            }
        }));
    });
}

/// `remove-artwork`: clear the preview and mark a Remove.
fn wire_remove_artwork(te: &TagEditor, ui: &AppWindow, session: &Rc<RefCell<TagSession>>) {
    let weak = ui.as_weak();
    let session = session.clone();
    te.on_remove_artwork(move || {
        let Some(ui) = weak.upgrade() else { return };
        let te = ui.global::<TagEditor>();
        te.set_cover(Image::default());
        te.set_has_cover(false);
        let mut sess = session.borrow_mut();
        sess.artwork = ArtworkEdit::Remove;
        sess.picked = None;
    });
}

/// `commit`: diff the form → `TagEdit` → apply → completion toast.
fn wire_commit(
    te: &TagEditor,
    ui: &AppWindow,
    state: &AppState,
    session: &Rc<RefCell<TagSession>>,
    notifications: &Rc<NotificationsUi>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    let session = session.clone();
    let notifications = notifications.clone();
    te.on_commit(move || {
        let Some(ui) = weak.upgrade() else { return };
        let te = ui.global::<TagEditor>();

        let (edit, ids, artwork_source) = {
            let sess = session.borrow();
            if sess.originals.len() != FIELD_COUNT {
                return;
            }
            // Read the live properties in canonical field order (TITLE..LYRICS).
            let cur = [
                te.get_title().to_string(),
                te.get_artist().to_string(),
                te.get_album_artist().to_string(),
                te.get_album().to_string(),
                te.get_genre().to_string(),
                te.get_year().to_string(),
                te.get_original_year().to_string(),
                te.get_track_number().to_string(),
                te.get_disc_number().to_string(),
                te.get_composer().to_string(),
                te.get_comment().to_string(),
                te.get_bpm().to_string(),
                te.get_lyrics().to_string(),
            ];
            let edit = build_edit(&cur, &sess.originals, sess.artwork.clone());
            (edit, sess.ids.clone(), sess.picked.clone())
        };

        // A reflexive open-then-Save must not touch disk (lofty rewrites the
        // tag whether or not anything changed). `is_noop()` already folds in
        // the artwork tri-state.
        if edit.is_noop() {
            return;
        }

        let s = state.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            let result = library::tags::apply_tag_edit(&s, ids, edit, artwork_source).await;
            let Some(ui) = weak.upgrade() else { return };
            show_report_toast(&ui, &notifications, result);
        }));
    });
}

/// Fill the `TagEditor` global from the fetched rows and record the snapshot.
fn populate(
    ui: &AppWindow,
    session: &Rc<RefCell<TagSession>>,
    rows: &[TagEditRow],
    lyrics: String,
    cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
) {
    let te = ui.global::<TagEditor>();
    let sentinel = ui.global::<Settings>().invoke_tag_multiple_values();

    let mut originals: Vec<String> = Vec::with_capacity(FIELD_COUNT);

    // Set one field: value + placeholder (the ‹multiple values› sentinel iff the
    // selection disagrees) + record the populated string as the diff baseline.
    macro_rules! field {
        ($val:expr, $set:ident, $set_ph:ident) => {{
            let (value, disagrees) = $val;
            te.$set(SharedString::from(value.as_str()));
            te.$set_ph(if disagrees {
                sentinel.clone()
            } else {
                SharedString::default()
            });
            originals.push(value);
        }};
    }

    field!(common_str(rows.iter().map(|r| r.title.as_str())), set_title, set_title_placeholder);
    field!(
        common_str(rows.iter().map(|r| r.artist.as_deref().unwrap_or_default())),
        set_artist,
        set_artist_placeholder
    );
    field!(
        common_str(rows.iter().map(|r| r.album_artist.as_deref().unwrap_or_default())),
        set_album_artist,
        set_album_artist_placeholder
    );
    field!(
        common_str(rows.iter().map(|r| r.album.as_deref().unwrap_or_default())),
        set_album,
        set_album_placeholder
    );
    field!(
        common_str(rows.iter().map(|r| r.genre.as_deref().unwrap_or_default())),
        set_genre,
        set_genre_placeholder
    );
    field!(
        common_by(rows.iter().map(|r| r.year), int_key, fmt_int),
        set_year,
        set_year_placeholder
    );
    field!(
        common_by(rows.iter().map(|r| r.original_year), int_key, fmt_int),
        set_original_year,
        set_original_year_placeholder
    );
    field!(
        common_by(rows.iter().map(|r| r.track_number), int_key, fmt_int),
        set_track_number,
        set_track_number_placeholder
    );
    field!(
        common_by(rows.iter().map(|r| r.disc_number), int_key, fmt_int),
        set_disc_number,
        set_disc_number_placeholder
    );
    field!(
        common_str(rows.iter().map(|r| r.composer.as_deref().unwrap_or_default())),
        set_composer,
        set_composer_placeholder
    );
    field!(
        common_str(rows.iter().map(|r| r.comment.as_deref().unwrap_or_default())),
        set_comment,
        set_comment_placeholder
    );
    field!(common_by(rows.iter().map(|r| r.bpm), bpm_key, fmt_bpm), set_bpm, set_bpm_placeholder);

    // Lyrics (single selection only; multi mode leaves it "" ⇒ Keep).
    te.set_lyrics(SharedString::from(lyrics.as_str()));
    te.set_lyrics_placeholder(SharedString::default());
    originals.push(lyrics);

    finalize_populate(&te, session, rows, originals, cover);
}

/// Finish a `populate`: scalar flags, cover, Summary, and the session snapshot.
fn finalize_populate(
    te: &TagEditor,
    session: &Rc<RefCell<TagSession>>,
    rows: &[TagEditRow],
    originals: Vec<String>,
    cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
) {
    let single = rows.len() == 1;
    te.set_track_count(clamp_i32(rows.len()));
    te.set_active_tab(0);
    te.set_lyrics_enabled(single);

    if let Some(buf) = cover {
        te.set_cover(Image::from_rgb8(buf));
        te.set_has_cover(true);
    } else {
        te.set_cover(Image::default());
        te.set_has_cover(false);
    }

    // Single row ⇒ fill the Summary tab; multi ⇒ blank it (tab unmounted).
    set_summary(te, single.then(|| &rows[0]));

    *session.borrow_mut() = TagSession {
        ids: rows.iter().map(|r| r.id).collect(),
        originals,
        artwork: ArtworkEdit::Keep,
        picked: None,
    };
}

/// Fill the read-only Summary strings from a single row's technical columns, or
/// blank all ten when `row` is `None` (multi mode — the tab isn't mounted, but a
/// stale single-select value shouldn't linger).
fn set_summary(te: &TagEditor, row: Option<&TagEditRow>) {
    // Each field is the row's value, or "" when `row` is None.
    fn s(row: Option<&TagEditRow>, get: impl Fn(&TagEditRow) -> String) -> SharedString {
        SharedString::from(row.map(get).unwrap_or_default().as_str())
    }
    te.set_summary_path(s(row, |r| r.file_path.clone()));
    te.set_summary_codec(s(row, |r| r.codec.as_deref().unwrap_or_default().to_uppercase()));
    te.set_summary_bitrate(s(row, |r| r.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default()));
    te.set_summary_sample_rate(s(row, |r| r.sample_rate.map(fmt_sample_rate).unwrap_or_default()));
    te.set_summary_bit_depth(s(row, |r| {
        r.bit_depth.map(|d| format!("{d}-bit")).unwrap_or_default()
    }));
    te.set_summary_channels(s(row, |r| r.channels.map(fmt_channels).unwrap_or_default()));
    te.set_summary_size(s(row, |r| r.file_size.map(fmt_size).unwrap_or_default()));
    te.set_summary_duration(s(row, |r| crate::ui::tracks::format_duration_ms(r.duration_ms)));
    te.set_summary_modified(s(row, |r| r.date_modified.as_deref().unwrap_or_default().to_owned()));
    te.set_summary_hash(s(row, |r| r.file_hash.as_deref().unwrap_or_default().to_owned()));
}

/// Show the Save completion toast from the report — a partial failure or an
/// unsupported field must be visible, not swallowed.
fn show_report_toast(
    ui: &AppWindow,
    notifications: &Rc<NotificationsUi>,
    result: Result<TagEditReport, AppError>,
) {
    let settings = ui.global::<Settings>();
    let report = match result {
        Ok(report) => report,
        Err(e) => {
            log::warn!("apply_tag_edit: {e}");
            notifications.show(failure_toast(&settings));
            return;
        }
    };

    if report.updated == 0 {
        notifications.show(failure_toast(&settings));
        return;
    }

    let failed = clamp_i32(report.failures.len());
    let unsupported = clamp_i32(report.unsupported.len());
    let variant = if failed == 0 && unsupported == 0 {
        "success"
    } else {
        "warning"
    };
    notifications.show_auto_dismiss(
        NotificationParams::plain(
            variant,
            settings.invoke_tag_edit_title(clamp_i32(report.updated)),
            settings.invoke_tag_edit_message(failed, unsupported),
        ),
        TOAST_MS,
    );
}

fn failure_toast(settings: &Settings) -> NotificationParams {
    NotificationParams::plain(
        "error",
        settings.invoke_tag_edit_failed_title(),
        settings.invoke_tag_edit_failed_message(),
    )
}

/// Build the `TagEdit` by diffing each current field value against the
/// populate-time snapshot. `cur` is fixed-length 13; `orig` is checked to match.
fn build_edit(cur: &[String], orig: &[String], artwork: ArtworkEdit) -> TagEdit {
    TagEdit {
        title: diff_str(&cur[TITLE], &orig[TITLE]),
        artist: diff_str(&cur[ARTIST], &orig[ARTIST]),
        album_artist: diff_str(&cur[ALBUM_ARTIST], &orig[ALBUM_ARTIST]),
        album: diff_str(&cur[ALBUM], &orig[ALBUM]),
        genre: diff_str(&cur[GENRE], &orig[GENRE]),
        year: diff_parsed::<u16>(&cur[YEAR], &orig[YEAR]),
        original_year: diff_parsed::<u16>(&cur[ORIGINAL_YEAR], &orig[ORIGINAL_YEAR]),
        track_number: diff_parsed::<u32>(&cur[TRACK_NUMBER], &orig[TRACK_NUMBER]),
        disc_number: diff_parsed::<u32>(&cur[DISC_NUMBER], &orig[DISC_NUMBER]),
        composer: diff_str(&cur[COMPOSER], &orig[COMPOSER]),
        comment: diff_str(&cur[COMMENT], &orig[COMMENT]),
        bpm: diff_bpm(&cur[BPM], &orig[BPM]),
        lyrics: diff_str(&cur[LYRICS], &orig[LYRICS]),
        artwork,
        // The Edit-Tags dialog doesn't surface MusicBrainz ids; leaving them
        // `Keep` preserves whatever the file (or the auto-tag backfill) wrote.
        ..Default::default()
    }
}

/// Tri-state for a text field. Exact match ⇒ `Keep`; a now-blank value ⇒
/// `Clear`; otherwise `Set` the value verbatim (the reader trims on the way
/// back in). An exact compare (not trimmed) preserves lyrics whitespace.
fn diff_str(cur: &str, orig: &str) -> FieldEdit<String> {
    if cur == orig {
        FieldEdit::Keep
    } else if cur.trim().is_empty() {
        FieldEdit::Clear
    } else {
        FieldEdit::Set(cur.to_owned())
    }
}

/// Tri-state for a numeric field that crosses the Slint boundary as its decimal
/// string. Unchanged ⇒ `Keep`; now-blank ⇒ `Clear`; a value that doesn't parse
/// degrades to `Keep` — never write garbage.
fn diff_parsed<T: std::str::FromStr>(cur: &str, orig: &str) -> FieldEdit<T> {
    if cur == orig {
        return FieldEdit::Keep;
    }
    let t = cur.trim();
    if t.is_empty() {
        return FieldEdit::Clear;
    }
    t.parse::<T>().map_or(FieldEdit::Keep, FieldEdit::Set)
}

fn diff_bpm(cur: &str, orig: &str) -> FieldEdit<f64> {
    if cur == orig {
        return FieldEdit::Keep;
    }
    let t = cur.trim();
    if t.is_empty() {
        return FieldEdit::Clear;
    }
    // `parse::<f64>()` accepts "nan" / "inf" — reject those and negatives so a
    // bogus tempo never reaches the writer.
    match t.parse::<f64>() {
        Ok(b) if b.is_finite() && b > 0.0 => FieldEdit::Set(b),
        _ => FieldEdit::Keep,
    }
}

/// Common value across the selection for a string field: `(value, disagrees)`.
/// All rows agree ⇒ that value; they differ ⇒ empty + the multi-value flag.
/// Borrows each row (`&str`) to compare and clones only the winner — vs cloning
/// every row's value just to test agreement.
fn common_str<'a>(mut values: impl Iterator<Item = &'a str>) -> (String, bool) {
    let Some(first) = values.next() else {
        return (String::new(), false);
    };
    if values.any(|v| v != first) {
        (String::new(), true)
    } else {
        (first.to_owned(), false)
    }
}

/// Common value across the selection for a formatted (numeric) field. Compares a
/// cheap `Copy + Eq` key so no per-row string is allocated to test agreement,
/// and formats only the single winning value. The key must collapse together
/// every value that *renders* identically (e.g. a `Some(0)` and a `None` year
/// both display empty ⇒ same key), so key equality matches display equality.
fn common_by<T: Copy, K: Eq>(
    mut values: impl Iterator<Item = T>,
    key: impl Fn(T) -> K,
    fmt: impl Fn(T) -> String,
) -> (String, bool) {
    let Some(first) = values.next() else {
        return (String::new(), false);
    };
    let first_key = key(first);
    if values.any(|v| key(v) != first_key) {
        (String::new(), true)
    } else {
        (fmt(first), false)
    }
}

/// Display-collapsing key for an integer field: non-positive / absent all render
/// empty (see [`fmt_int`]), so map them to one `None` bucket.
fn int_key(v: Option<i32>) -> Option<i32> {
    v.filter(|&n| n > 0)
}

/// Display-collapsing key for BPM: non-finite / non-positive render empty (see
/// [`fmt_bpm`]); the finite-positive values are keyed by their bit pattern so
/// equality is exact without a float `==` (which the pedantic gate rejects).
fn bpm_key(v: Option<f64>) -> Option<u64> {
    v.filter(|b| b.is_finite() && *b > 0.0).map(f64::to_bits)
}

/// `Option<i32>` → display string; empty for absent / non-positive (0 is "unset"
/// for year / track / disc).
fn fmt_int(v: Option<i32>) -> String {
    match v {
        Some(n) if n > 0 => n.to_string(),
        _ => String::new(),
    }
}

fn fmt_bpm(v: Option<f64>) -> String {
    match v {
        Some(b) if b.is_finite() && b > 0.0 => {
            if b.fract().abs() < f64::EPSILON {
                format!("{b:.0}")
            } else {
                format!("{b}")
            }
        }
        _ => String::new(),
    }
}

/// Hz → "44.1 kHz" / "48 kHz" (mirrors `now_playing::metadata::format_sample_rate`).
fn fmt_sample_rate(hz: i32) -> String {
    let khz = f64::from(hz) / 1000.0;
    if khz.fract().abs() < f64::EPSILON {
        format!("{khz:.0} kHz")
    } else {
        format!("{khz:.1} kHz")
    }
}

fn fmt_channels(n: i32) -> String {
    match n {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        n => format!("{n} channels"),
    }
}

/// Human byte size (integer math only — avoids the `cast_precision_loss` an
/// `i64 as f64` would trip under the pedantic gate).
fn fmt_size(bytes: i64) -> String {
    const KIB: i64 = 1024;
    const MIB: i64 = 1024 * KIB;
    const GIB: i64 = 1024 * MIB;
    let b = bytes.max(0);
    if b < KIB {
        format!("{b} B")
    } else if b < MIB {
        format!("{} KB", b / KIB)
    } else if b < GIB {
        format!("{}.{} MB", b / MIB, (b % MIB) * 10 / MIB)
    } else {
        format!("{}.{} GB", b / GIB, (b % GIB) * 10 / GIB)
    }
}

fn clamp_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Decode a cover source into a bounded RGB buffer for the dialog preview.
/// Blocking (image decode) — call under `spawn_blocking`. `None` on any decode
/// error: the preview is best-effort, and the write path re-validates the pick.
fn decode_cover_preview(path: &Path) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
    // The dialog tile renders at 160 px, so the shared 384 px cover tier keeps
    // it crisp on HiDPI while staying a small bounded buffer.
    let decoded = decode_capped(path, MAX_SOURCE_DIM).ok()?;
    let (width, height) = fit_within(decoded.width(), decoded.height(), COVER_SIZE, COVER_SIZE);
    let rgb = resize_rgb8(&decoded, width, height, FilterType::Box)?;
    Some(buffer_from_rgb(&rgb))
}

#[cfg(test)]
#[path = "tests/tags_tests.rs"]
mod tests;
