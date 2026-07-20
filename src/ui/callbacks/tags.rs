//! Edit-Track-Information dialog callbacks.
//!
//! The `TagEditor` global (`ui/globals.slint`) is Rust-owned: `request-edit`
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
//! sqlx / `spawn_blocking` calls, exactly as `wire_playlist_files` does. Wired
//! from `main.rs` after the notifications stack exists, for the same reason.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, Model, Rgb8Pixel, SharedPixelBuffer, SharedString};

use crate::database::queries;
use crate::entities::track::TagEditRow;
use crate::error::AppError;
use crate::library;
use crate::library::tags::TagEditReport;
use crate::media::tag_writer::{self, ArtworkEdit, FieldEdit, TagEdit};
use crate::state::AppState;
use crate::ui::notifications::{NotificationParams, NotificationsUi};
use crate::{AppWindow, Dialog, Settings, TagEditor};

/// The editable fields, in the order [`build_edit`] / `TagSession::originals`
/// index them: title..bpm then lyrics.
const FIELD_COUNT: usize = 13;

/// Preview cover cap — the dialog tile renders at 160 px; 384 keeps it crisp on
/// `HiDPI` while staying a small bounded buffer (mirrors `detail_artwork`).
const PREVIEW_SIZE: u32 = 384;

/// Reject absurd source dimensions before allocating (forged-header guard),
/// the same bound `CoverThumbs` / `detail_artwork` use.
const MAX_SOURCE_DIM: u32 = 8192;

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
/// stack exists (same constraint as `wire_playlist_files`).
pub fn wire_tags(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let session: Rc<RefCell<TagSession>> = Rc::new(RefCell::new(TagSession::default()));
    let te = ui.global::<TagEditor>();

    // request-edit: fetch rows (+ lyrics + cover for a single selection),
    // populate, and open on the next UI tick.
    {
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
                let rows = match queries::track::get_tag_edit_rows_by_ids(&s.db, &ids).await {
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
                    match s
                        .runtime
                        .spawn_blocking(move || tag_writer::read_lyrics(&path))
                        .await
                    {
                        Ok(Ok(Some(l))) => l,
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };

                // Cover preview from the first row that has one (decode off the
                // UI thread — a full-res source could jank it).
                let cover_path = rows.iter().find_map(|r| {
                    r.artwork_path
                        .as_deref()
                        .filter(|p| !p.is_empty())
                        .map(PathBuf::from)
                });
                let cover = match cover_path {
                    Some(p) => s
                        .runtime
                        .spawn_blocking(move || decode_cover_preview(&p))
                        .await
                        .ok()
                        .flatten(),
                    None => None,
                };

                let Some(ui) = weak.upgrade() else { return };
                populate(&ui, &session, &rows, lyrics, cover);
                ui.global::<Dialog>().set_open(true);
            }));
        });
    }

    // pick-artwork: native image picker → decode preview → stash as a Replace.
    {
        let weak = ui.as_weak();
        let state = state.clone();
        let session = session.clone();
        te.on_pick_artwork(move || {
            let weak = weak.clone();
            let s = state.clone();
            let session = session.clone();
            let _ = slint::spawn_local(Compat::new(async move {
                let dialog = {
                    // Filter broadly — the orchestrator normalizes on write
                    // (lofty's accepted set and MP4's differ; no single filter
                    // expresses it).
                    let mut d = rfd::AsyncFileDialog::new()
                        .set_title("Choose Cover Image")
                        .add_filter(
                            "Images",
                            &["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"],
                        );
                    if let Some(ui) = weak.upgrade() {
                        d = d.set_parent(&ui.window().window_handle());
                    }
                    d
                };
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

    // remove-artwork: clear the preview and mark a Remove.
    {
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

    // commit: diff the form → TagEdit → apply → completion toast.
    {
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
            // tag whether or not anything changed).
            if edit.is_noop() && edit.artwork == ArtworkEdit::Keep {
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
    let single = rows.len() == 1;

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

    field!(common(rows.iter().map(|r| r.title.clone())), set_title, set_title_placeholder);
    field!(common(rows.iter().map(|r| opt(r.artist.as_ref()))), set_artist, set_artist_placeholder);
    field!(
        common(rows.iter().map(|r| opt(r.album_artist.as_ref()))),
        set_album_artist,
        set_album_artist_placeholder
    );
    field!(common(rows.iter().map(|r| opt(r.album.as_ref()))), set_album, set_album_placeholder);
    field!(common(rows.iter().map(|r| opt(r.genre.as_ref()))), set_genre, set_genre_placeholder);
    field!(common(rows.iter().map(|r| fmt_int(r.year))), set_year, set_year_placeholder);
    field!(
        common(rows.iter().map(|r| fmt_int(r.original_year))),
        set_original_year,
        set_original_year_placeholder
    );
    field!(
        common(rows.iter().map(|r| fmt_int(r.track_number))),
        set_track_number,
        set_track_number_placeholder
    );
    field!(
        common(rows.iter().map(|r| fmt_int(r.disc_number))),
        set_disc_number,
        set_disc_number_placeholder
    );
    field!(
        common(rows.iter().map(|r| opt(r.composer.as_ref()))),
        set_composer,
        set_composer_placeholder
    );
    field!(common(rows.iter().map(|r| opt(r.comment.as_ref()))), set_comment, set_comment_placeholder);
    field!(common(rows.iter().map(|r| fmt_bpm(r.bpm))), set_bpm, set_bpm_placeholder);

    // Lyrics (single selection only; multi mode leaves it "" ⇒ Keep).
    te.set_lyrics(SharedString::from(lyrics.as_str()));
    te.set_lyrics_placeholder(SharedString::default());
    originals.push(lyrics);

    te.set_track_count(i32::try_from(rows.len()).unwrap_or(i32::MAX));
    te.set_active_tab(0);
    te.set_lyrics_enabled(single);

    if let Some(buf) = cover {
        te.set_cover(Image::from_rgb8(buf));
        te.set_has_cover(true);
    } else {
        te.set_cover(Image::default());
        te.set_has_cover(false);
    }

    if single {
        set_summary(&te, &rows[0]);
    } else {
        clear_summary(&te);
    }

    *session.borrow_mut() = TagSession {
        ids: rows.iter().map(|r| r.id).collect(),
        originals,
        artwork: ArtworkEdit::Keep,
        picked: None,
    };
}

/// Preformat the read-only Summary strings from a single row's technical columns.
fn set_summary(te: &TagEditor, r: &TagEditRow) {
    te.set_summary_path(SharedString::from(r.file_path.as_str()));
    te.set_summary_codec(SharedString::from(
        r.codec.as_deref().unwrap_or_default().to_uppercase(),
    ));
    te.set_summary_bitrate(SharedString::from(
        r.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default(),
    ));
    te.set_summary_sample_rate(SharedString::from(
        r.sample_rate.map(fmt_sample_rate).unwrap_or_default(),
    ));
    te.set_summary_bit_depth(SharedString::from(
        r.bit_depth.map(|d| format!("{d}-bit")).unwrap_or_default(),
    ));
    te.set_summary_channels(SharedString::from(
        r.channels.map(fmt_channels).unwrap_or_default(),
    ));
    te.set_summary_size(SharedString::from(
        r.file_size.map(fmt_size).unwrap_or_default(),
    ));
    te.set_summary_duration(SharedString::from(crate::ui::tracks::format_duration_ms(
        r.duration_ms,
    )));
    te.set_summary_modified(SharedString::from(
        r.date_modified.as_deref().unwrap_or_default(),
    ));
    te.set_summary_hash(SharedString::from(r.file_hash.as_deref().unwrap_or_default()));
}

/// Blank the Summary strings (multi mode — the tab isn't mounted, but stale
/// single-select values shouldn't linger).
fn clear_summary(te: &TagEditor) {
    let empty = SharedString::default();
    te.set_summary_path(empty.clone());
    te.set_summary_codec(empty.clone());
    te.set_summary_bitrate(empty.clone());
    te.set_summary_sample_rate(empty.clone());
    te.set_summary_bit_depth(empty.clone());
    te.set_summary_channels(empty.clone());
    te.set_summary_size(empty.clone());
    te.set_summary_duration(empty.clone());
    te.set_summary_modified(empty.clone());
    te.set_summary_hash(empty);
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
        NotificationParams {
            variant: variant.into(),
            title: settings.invoke_tag_edit_title(clamp_i32(report.updated)),
            message: settings.invoke_tag_edit_message(failed, unsupported),
            action_label: SharedString::default(),
            action_kind: SharedString::default(),
        },
        TOAST_MS,
    );
}

fn failure_toast(settings: &Settings) -> NotificationParams {
    NotificationParams {
        variant: "error".into(),
        title: settings.invoke_tag_edit_failed_title(),
        message: settings.invoke_tag_edit_failed_message(),
        action_label: SharedString::default(),
        action_kind: SharedString::default(),
    }
}

/// Build the `TagEdit` by diffing each current field value against the
/// populate-time snapshot. `cur` is fixed-length 13; `orig` is checked to match.
fn build_edit(cur: &[String], orig: &[String], artwork: ArtworkEdit) -> TagEdit {
    TagEdit {
        title: diff_str(&cur[0], &orig[0]),
        artist: diff_str(&cur[1], &orig[1]),
        album_artist: diff_str(&cur[2], &orig[2]),
        album: diff_str(&cur[3], &orig[3]),
        genre: diff_str(&cur[4], &orig[4]),
        year: diff_u16(&cur[5], &orig[5]),
        original_year: diff_u16(&cur[6], &orig[6]),
        track_number: diff_u32(&cur[7], &orig[7]),
        disc_number: diff_u32(&cur[8], &orig[8]),
        composer: diff_str(&cur[9], &orig[9]),
        comment: diff_str(&cur[10], &orig[10]),
        bpm: diff_bpm(&cur[11], &orig[11]),
        lyrics: diff_str(&cur[12], &orig[12]),
        artwork,
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

fn diff_u16(cur: &str, orig: &str) -> FieldEdit<u16> {
    if cur == orig {
        return FieldEdit::Keep;
    }
    let t = cur.trim();
    if t.is_empty() {
        return FieldEdit::Clear;
    }
    // A value that doesn't parse degrades to Keep — never write garbage.
    t.parse::<u16>().map_or(FieldEdit::Keep, FieldEdit::Set)
}

fn diff_u32(cur: &str, orig: &str) -> FieldEdit<u32> {
    if cur == orig {
        return FieldEdit::Keep;
    }
    let t = cur.trim();
    if t.is_empty() {
        return FieldEdit::Clear;
    }
    t.parse::<u32>().map_or(FieldEdit::Keep, FieldEdit::Set)
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

/// Common value across the selection: `(value, disagrees)`. All rows agree ⇒
/// that value; they differ ⇒ empty + the multi-value flag.
fn common(mut values: impl Iterator<Item = String>) -> (String, bool) {
    let Some(first) = values.next() else {
        return (String::new(), false);
    };
    if values.any(|v| v != first) {
        (String::new(), true)
    } else {
        (first, false)
    }
}

fn opt(o: Option<&String>) -> String {
    o.cloned().unwrap_or_default()
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
    let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);
    reader.limits(limits);

    let rgb = reader.decode().ok()?.thumbnail(PREVIEW_SIZE, PREVIEW_SIZE).to_rgb8();
    let (w, h) = rgb.dimensions();
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(rgb.as_raw());
    Some(buf)
}

#[cfg(test)]
#[path = "tests/tags_tests.rs"]
mod tests;
