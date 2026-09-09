//! Edit-Track-Information dialog callbacks.
//!
//! The `TagEditor` global (`crates/melodia-ui/ui/globals/dialog-forms.slint`) is Rust-owned: `request-edit`
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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use async_compat::Compat;
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use crate::ui::file_dialog;
use crate::ui::shell::notifications::{NotificationParams, NotificationsUi, RowText};
use crate::ui::util::{COVER_SIZE, buffer_from_rgb};
use melodia_app::library;
use melodia_app::library::tags::TagEditReport;
use melodia_app::state::AppState;
use melodia_artwork::media::image::image_decode::{
    FilterType, MAX_SOURCE_DIM, decode_capped, fit_within, resize_rgb8,
};
use melodia_core::entities::artist::{ArtistCredit, CreditedArtist, JOIN_PHRASES};
use melodia_core::entities::tags::{ArtworkEdit, FieldEdit, TagEdit};
use melodia_core::entities::track::TagEditRow;
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, ArtistCreditRow, Dialog, Settings, TagEditor};

/// Canonical field order, shared by the three positional lists that must stay
/// aligned: the `commit` getter array, `populate`'s `field!` calls, and
/// [`build_edit`] / `TagSession::originals`. Indexing by name (not raw `0..12`)
/// makes the alignment explicit — reorder here and in all three sites together.
const TITLE: usize = 0;
const ALBUM: usize = 1;
const GENRE: usize = 2;
const YEAR: usize = 3;
const ORIGINAL_YEAR: usize = 4;
const TRACK_NUMBER: usize = 5;
const DISC_NUMBER: usize = 6;
const COMPOSER: usize = 7;
const COMMENT: usize = 8;
const BPM: usize = 9;
const LYRICS: usize = 10;

/// Number of editable *string* fields (`title`..`lyrics`). The two artist fields are credits and
/// diff structurally, so they sit outside this array rather than in it as rendered lines.
const FIELD_COUNT: usize = LYRICS + 1;

/// Which credit a `TagEditor` row callback names, mirroring the global's own `field-artist` /
/// `field-album-artist`. Two spellings of one position is what drifts, so the Slint side reads
/// its from the global rather than restating the number.
const CREDIT_ARTIST: usize = 0;
const CREDIT_ALBUM_ARTIST: usize = 1;
const CREDIT_FIELD_COUNT: usize = 2;

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
    /// The credit each artist field was populated with. Structural rather than a rendered line,
    /// because two different credits can render the same string and only one of them is what the
    /// user is looking at.
    original_credits: [ArtistCredit; CREDIT_FIELD_COUNT],
    /// Picker label paired with what it renders as, in the order the Slint `[string]` holds them.
    /// Seeded from `JOIN_PHRASES` and extended by whatever the opened files already use.
    join_phrases: Vec<(String, String)>,
    artwork: ArtworkEdit,
    /// The picked cover path — set only while `artwork == Replace`. Rides to the
    /// orchestrator as `apply_tag_edit`'s separate `artwork_source` arg.
    picked: Option<PathBuf>,
    /// The sheet this track already has somewhere other than its own tag, held from the open so
    /// the Insert button costs no read of its own. `None` where there is nothing to offer.
    resident_lyrics: Option<String>,
}

/// Wire the four `TagEditor` callbacks. Needs `Rc<NotificationsUi>` for the
/// Save completion toast, so it is called from `main.rs` after the notifications
/// stack exists (same constraint as `ui::playlists::wire_files`).
pub fn wire_tags(ui: &AppWindow, state: &AppState, notifications: &Rc<NotificationsUi>) {
    let session: Rc<RefCell<TagSession>> = Rc::new(RefCell::new(TagSession::default()));
    let te = ui.global::<TagEditor>();

    wire_request_edit(&te, ui, state, &session);
    wire_credits(&te, ui, &session);
    wire_insert_resident_lyrics(&te, ui, &session);
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
    te.on_request_edit(move |ids_model, tab| {
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

            // Lyrics live in the file, not the DB, and so does the authoritative credit — a
            // library whose join rows were seeded from `artist_id` alone knows only the first
            // name. One read for both; a multi-selection takes the database instead, N file reads
            // on the open path being what the row projection exists to avoid.
            let (lyrics, credits) = if single {
                let path = PathBuf::from(&rows[0].file_path);
                let read = s.runtime.spawn_blocking(move || {
                    (library::tags::read_lyrics(&path), library::tags::read_credits(&path))
                });
                match read.await {
                    Ok((lyrics, credits)) => (
                        lyrics.ok().flatten().unwrap_or_default(),
                        vec![credits.unwrap_or_default()],
                    ),
                    Err(e) => {
                        log::warn!("tag edit: reading {} failed: {e}", rows[0].file_path);
                        (String::new(), vec![<(ArtistCredit, ArtistCredit)>::default()])
                    }
                }
            } else {
                let by_id =
                    library::tags::get_tag_edit_credits(&s, &ids).await.unwrap_or_else(|e| {
                        log::warn!("tag edit credits: {}", describe(&e));
                        HashMap::new()
                    });
                let credits =
                    rows.iter().map(|r| by_id.get(&r.id).cloned().unwrap_or_default()).collect();
                (String::new(), credits)
            };

            // What the Now Playing panel would show for this track, which is the tag only when
            // nothing better exists. Offered rather than applied: writing it is a tag edit like
            // any other, so it goes through this dialog's own Save.
            let resident = if single {
                library::lyrics::resident_text(&s, &rows[0].file_path)
                    .await
                    .unwrap_or_else(|e| {
                        log::debug!("tag edit: no resident sheet: {}", describe(&e));
                        None
                    })
                    .filter(|text| text.trim() != lyrics.trim())
            } else {
                None
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
            populate(&ui, &session, &rows, &credits, lyrics, resident, cover);
            // After `populate`, which resets to Tags: the request's tab is the last word. Only a
            // single selection can honour it, Lyrics and Summary being unmounted in batch mode, so
            // a request for one over many rows would open on a tab that draws nothing. The bounds
            // are the global's own, so nothing here restates a position the body owns.
            if single {
                let te = ui.global::<TagEditor>();
                te.set_active_tab(tab.clamp(te.get_tab_tags(), te.get_tab_summary()));
            }
            ui.global::<Dialog>().set_open(true);
        }));
    });
}

/// The four credit-row callbacks, plus the models they mutate.
///
/// Rust owns both models: a row view reports an edit and never writes what it was handed, which
/// is what lets the `Dropdown`s bind one-way and keep re-reading after a removal shifts every
/// index below it.
fn wire_credits(te: &TagEditor, ui: &AppWindow, session: &Rc<RefCell<TagSession>>) {
    // Installed once, and never replaced: a default-constructed `ModelRc` is a no-op model that
    // silently ignores `set_vec`, so `populate` needs a real one to be waiting for it.
    te.set_artists(ModelRc::new(VecModel::from(vec![blank_credit_row()])));
    te.set_album_artists(ModelRc::new(VecModel::from(vec![blank_credit_row()])));

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_add_credit(move |field| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        with_credits_model(&ui, field, |vm| vm.push(blank_credit_row()));
        refresh_preview(&ui, &sess, field);
    });

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_remove_credit(move |field, row| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        with_credits_model(&ui, field, |vm| {
            if let Ok(row) = usize::try_from(row)
                && row < vm.row_count()
            {
                vm.remove(row);
            }
            // A credit with no names is a state the editor should not be able to reach: the
            // field would read as "cleared" on save, which is not what removing a collaborator
            // means.
            if vm.row_count() == 0 {
                vm.push(blank_credit_row());
            }
        });
        refresh_preview(&ui, &sess, field);
    });

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_set_credit_name(move |field, row, name| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        patch_credit_row(&ui, field, row, |mut r| {
            r.name = name;
            r
        });
        refresh_preview(&ui, &sess, field);
    });

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_set_credit_join(move |field, row, phrase| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        patch_credit_row(&ui, field, row, |mut r| {
            r.join_index = phrase;
            r
        });
        refresh_preview(&ui, &sess, field);
    });
}

/// Clamp a Slint-side field discriminator onto one of the two credits.
fn credit_field(field: i32) -> usize {
    if usize::try_from(field).unwrap_or(CREDIT_ARTIST) == CREDIT_ALBUM_ARTIST {
        CREDIT_ALBUM_ARTIST
    } else {
        CREDIT_ARTIST
    }
}

fn blank_credit_row() -> ArtistCreditRow {
    ArtistCreditRow {
        name: SharedString::new(),
        join_index: 0,
    }
}

fn with_credits_model<R>(
    ui: &AppWindow,
    field: usize,
    f: impl FnOnce(&VecModel<ArtistCreditRow>) -> R,
) -> Option<R> {
    let te = ui.global::<TagEditor>();
    let model = if field == CREDIT_ALBUM_ARTIST {
        te.get_album_artists()
    } else {
        te.get_artists()
    };
    model.as_any().downcast_ref::<VecModel<ArtistCreditRow>>().map(f)
}

fn patch_credit_row(
    ui: &AppWindow,
    field: usize,
    row: i32,
    f: impl FnOnce(ArtistCreditRow) -> ArtistCreditRow,
) {
    with_credits_model(ui, field, |vm| {
        if let Ok(row) = usize::try_from(row)
            && let Some(old) = vm.row_data(row)
        {
            vm.set_row_data(row, f(old));
        }
    });
}

/// Re-render the credit line under the rows after every edit, so the user sees what will be
/// written rather than having to picture the join phrases in place.
fn refresh_preview(ui: &AppWindow, session: &Rc<RefCell<TagSession>>, field: usize) {
    let credit = credit_from_model(ui, field, &session.borrow().join_phrases);
    let line = SharedString::from(credit.line().unwrap_or_default());
    let te = ui.global::<TagEditor>();
    if field == CREDIT_ALBUM_ARTIST {
        te.set_album_artist_preview(line);
    } else {
        te.set_artist_preview(line);
    }
}

/// The credit the rows currently spell.
///
/// Blank names drop out, so a row added and left empty writes nothing, and the **last** surviving
/// name loses its phrase whatever its picker says — there is nothing after it to join to, and a
/// trailing " feat. " in the file is exactly the kind of thing nobody notices until another
/// player shows it.
fn credit_from_model(ui: &AppWindow, field: usize, phrases: &[(String, String)]) -> ArtistCredit {
    let rows =
        with_credits_model(ui, field, |vm| vm.iter().collect::<Vec<_>>()).unwrap_or_default();
    let named: Vec<ArtistCreditRow> =
        rows.into_iter().filter(|r| !r.name.trim().is_empty()).collect();
    let last = named.len().saturating_sub(1);
    let artists = named
        .iter()
        .enumerate()
        .map(|(i, r)| CreditedArtist {
            name: r.name.trim().to_owned(),
            join_phrase: if i == last {
                String::new()
            } else {
                phrase_at(phrases, r.join_index)
            },
        })
        .collect();
    ArtistCredit::new(artists)
}

fn phrase_at(phrases: &[(String, String)], index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|i| phrases.get(i))
        .map_or_else(String::new, |(_, rendered)| rendered.clone())
}

/// The rows for one credit, registering any phrase the built-in list doesn't have.
fn rows_from_credit(
    credit: &ArtistCredit,
    phrases: &mut Vec<(String, String)>,
) -> Vec<ArtistCreditRow> {
    let rows: Vec<ArtistCreditRow> = credit
        .artists()
        .iter()
        .map(|a| ArtistCreditRow {
            name: SharedString::from(a.name.as_str()),
            join_index: register_phrase(phrases, &a.join_phrase),
        })
        .collect();
    if rows.is_empty() {
        vec![blank_credit_row()]
    } else {
        rows
    }
}

/// The picker index for `rendered`, appending it when the file uses a phrase the built-in list
/// has never heard of.
///
/// Round-tripping somebody else's " meets " matters more than a tidy picker: without this, opening
/// such a file and saving any field at all would silently rewrite the credit to whatever sits at
/// index 0.
fn register_phrase(phrases: &mut Vec<(String, String)>, rendered: &str) -> i32 {
    if rendered.is_empty() {
        return 0;
    }
    if let Some(i) = phrases.iter().position(|(_, known)| known == rendered) {
        return i32::try_from(i).unwrap_or(0);
    }
    phrases.push((rendered.trim().to_owned(), rendered.to_owned()));
    i32::try_from(phrases.len() - 1).unwrap_or(0)
}

/// Common credit across the selection: `(credit, disagrees)`, [`common_str`]'s shape.
fn common_credit<'a>(mut credits: impl Iterator<Item = &'a ArtistCredit>) -> (ArtistCredit, bool) {
    let Some(first) = credits.next() else {
        return (ArtistCredit::default(), false);
    };
    if credits.all(|c| c == first) {
        (first.clone(), false)
    } else {
        (ArtistCredit::default(), true)
    }
}

/// `insert-resident-lyrics`: fill the field with the sheet this track already has.
///
/// **Fills the box and stops there.** The write is the dialog's own Save, so the inserted sheet is
/// a diff against the populate-time baseline like every other field — which is what makes it
/// reviewable, cancellable, and undoable by the same Cancel that covers a mistyped title.
fn wire_insert_resident_lyrics(te: &TagEditor, ui: &AppWindow, session: &Rc<RefCell<TagSession>>) {
    let weak = ui.as_weak();
    let session = session.clone();
    te.on_insert_resident_lyrics(move || {
        let Some(ui) = weak.upgrade() else { return };
        let Some(text) = session.borrow().resident_lyrics.clone() else {
            return;
        };
        ui.global::<TagEditor>().set_lyrics(SharedString::from(text));
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
            let credits = [
                credit_from_model(&ui, CREDIT_ARTIST, &sess.join_phrases),
                credit_from_model(&ui, CREDIT_ALBUM_ARTIST, &sess.join_phrases),
            ];
            let edit = build_edit(
                &cur,
                &sess.originals,
                &credits,
                &sess.original_credits,
                sess.artwork.clone(),
            );
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
    credits: &[(ArtistCredit, ArtistCredit)],
    lyrics: String,
    resident: Option<String>,
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

    // The two credits. `common_credit` decides the ‹multiple values› sentinel the way `common_str`
    // does, and the hint rides on the first row's own input rather than a field-wide placeholder.
    let mut phrases: Vec<(String, String)> = JOIN_PHRASES
        .iter()
        .map(|(label, rendered)| ((*label).to_owned(), (*rendered).to_owned()))
        .collect();
    let (artist, artist_disagrees) = common_credit(credits.iter().map(|(a, _)| a));
    let (album_artist, album_artist_disagrees) = common_credit(credits.iter().map(|(_, a)| a));
    let artist_rows = rows_from_credit(&artist, &mut phrases);
    let album_artist_rows = rows_from_credit(&album_artist, &mut phrases);

    te.set_join_phrases(ModelRc::new(VecModel::from(
        phrases.iter().map(|(label, _)| SharedString::from(label.as_str())).collect::<Vec<_>>(),
    )));
    write_credit_rows(&te, CREDIT_ARTIST, artist_rows);
    write_credit_rows(&te, CREDIT_ALBUM_ARTIST, album_artist_rows);
    te.set_artist_placeholder(if artist_disagrees {
        sentinel.clone()
    } else {
        SharedString::default()
    });
    te.set_album_artist_placeholder(if album_artist_disagrees {
        sentinel.clone()
    } else {
        SharedString::default()
    });
    te.set_artist_preview(SharedString::from(artist.line().unwrap_or_default()));
    te.set_album_artist_preview(SharedString::from(album_artist.line().unwrap_or_default()));
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

    finalize_populate(
        &te,
        session,
        rows,
        originals,
        [artist, album_artist],
        phrases,
        resident,
        cover,
    );
}

/// Replace one credit's rows in place. The model is installed once by `wire_credits`, so this
/// refills it rather than handing over a new one.
fn write_credit_rows(te: &TagEditor, field: usize, rows: Vec<ArtistCreditRow>) {
    let model = if field == CREDIT_ALBUM_ARTIST {
        te.get_album_artists()
    } else {
        te.get_artists()
    };
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<ArtistCreditRow>>() {
        vm.set_vec(rows);
    }
}

/// Finish a `populate`: scalar flags, cover, Summary, and the session snapshot.
fn finalize_populate(
    te: &TagEditor,
    session: &Rc<RefCell<TagSession>>,
    rows: &[TagEditRow],
    originals: Vec<String>,
    original_credits: [ArtistCredit; CREDIT_FIELD_COUNT],
    join_phrases: Vec<(String, String)>,
    resident: Option<String>,
    cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
) {
    let single = rows.len() == 1;
    te.set_track_count(clamp_i32(rows.len()));
    te.set_active_tab(te.get_tab_tags());
    te.set_lyrics_enabled(single);
    te.set_has_resident_lyrics(resident.is_some());

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
        original_credits,
        join_phrases,
        artwork: ArtworkEdit::Keep,
        picked: None,
        resident_lyrics: resident,
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
            show_failure_toast(ui, notifications);
            return;
        }
    };

    if report.updated == 0 {
        show_failure_toast(ui, notifications);
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

/// Sticky, hence the recipe: a row still up when the language changes has to follow it.
fn show_failure_toast(ui: &AppWindow, notifications: &NotificationsUi) {
    notifications.show_localized(ui, "error", "", |ui| {
        let g = ui.global::<Settings>();
        RowText::plain(g.invoke_tag_edit_failed_title(), g.invoke_tag_edit_failed_message())
    });
}

/// Build the `TagEdit` by diffing each current field value against the
/// populate-time snapshot. `cur` is fixed-length [`FIELD_COUNT`]; `orig` is checked to match.
fn build_edit(
    cur: &[String],
    orig: &[String],
    credits: &[ArtistCredit; CREDIT_FIELD_COUNT],
    original_credits: &[ArtistCredit; CREDIT_FIELD_COUNT],
    artwork: ArtworkEdit,
) -> TagEdit {
    TagEdit {
        title: diff_str(&cur[TITLE], &orig[TITLE]),
        artist: diff_credit(&credits[CREDIT_ARTIST], &original_credits[CREDIT_ARTIST]),
        album_artist: diff_credit(
            &credits[CREDIT_ALBUM_ARTIST],
            &original_credits[CREDIT_ALBUM_ARTIST],
        ),
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

/// Tri-state for an artist field, compared structurally.
///
/// Not through the rendered line: two different credits can render the same string, and a phrase
/// swapped for one that renders identically is still an edit the file should receive. `Keep` is
/// what makes an untouched multi-artist file safe, the list tag surviving a save that never
/// looked at it.
fn diff_credit(cur: &ArtistCredit, orig: &ArtistCredit) -> FieldEdit<ArtistCredit> {
    if cur == orig {
        FieldEdit::Keep
    } else if cur.is_empty() {
        FieldEdit::Clear
    } else {
        FieldEdit::Set(cur.clone())
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
