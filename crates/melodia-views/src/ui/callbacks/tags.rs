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
use melodia_core::entities::album::ReleaseTagRow;
use melodia_core::entities::artist::{ArtistCredit, CreditedArtist, JOIN_PHRASES};
use melodia_core::entities::credits::{CreditRole, ROLES, RoleCredit, RoleCredits};
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::tags::{ArtworkEdit, FieldEdit, TagEdit};
use melodia_core::entities::track::TagEditRow;
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, ArtistCreditRow, Dialog, Settings, TagEditor};

/// Every editable field that crosses the boundary as a string, named rather than numbered.
///
/// This replaced three positional lists — a getter array, `populate`'s `field!` calls and the
/// diff — that had to agree on an index per field. At eleven fields that was merely fragile; the
/// vocabulary is thirty-odd now, and the failure mode is silent and destructive: an index off by
/// one writes the user's ISRC into their mood tag. Named on both sides of the diff, a mistake is a
/// compile error.
///
/// The two artist fields are absent on purpose — they are credits and diff structurally, not as
/// the rendered lines this holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TextFields {
    title: String,
    album: String,
    year: String,
    original_year: String,
    track_number: String,
    track_total: String,
    disc_number: String,
    disc_total: String,
    comment: String,
    bpm: String,
    subtitle: String,
    disc_subtitle: String,
    grouping: String,
    work: String,
    movement: String,
    movement_number: String,
    movement_total: String,
    initial_key: String,
    mood: String,
    language: String,
    isrc: String,
    copyright: String,
    label: String,
    catalog_number: String,
    barcode: String,
    media: String,
    release_type: String,
    release_country: String,
    lyrics: String,
}

/// The whole editable form: the text fields plus the one switch among them.
///
/// The switch rides here rather than as its own argument so "what the user was shown" and "what
/// the user left behind" are one type, compared as a whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FormState {
    text: TextFields,
    compilation: bool,
}

/// The four multi-value fields, which every format worth the name stores as a list.
///
/// Their own type because they share a contract the text fields don't: each is edited as *rows*
/// and diffed **structurally**, never through the string it renders as. Two credits that render
/// alike are still different credits, and a genre may contain the comma its column joins with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ListFields {
    credits: [ArtistCredit; CREDIT_FIELD_COUNT],
    genres: GenreList,
    roles: RoleCredits,
}

/// The live form, read back off the globals in one place.
///
/// Its counterpart is `populate`'s `field!`, which writes each of these and records the same value
/// as the baseline — so the two sites name the same field and nothing pairs them by position.
fn read_form(te: &TagEditor) -> FormState {
    FormState {
        compilation: te.get_compilation(),
        text: read_text_fields(te),
    }
}

fn read_text_fields(te: &TagEditor) -> TextFields {
    TextFields {
        title: te.get_title().to_string(),
        album: te.get_album().to_string(),
        year: te.get_year().to_string(),
        original_year: te.get_original_year().to_string(),
        track_number: te.get_track_number().to_string(),
        track_total: te.get_track_total().to_string(),
        disc_number: te.get_disc_number().to_string(),
        disc_total: te.get_disc_total().to_string(),
        comment: te.get_comment().to_string(),
        bpm: te.get_bpm().to_string(),
        subtitle: te.get_subtitle().to_string(),
        disc_subtitle: te.get_disc_subtitle().to_string(),
        grouping: te.get_grouping().to_string(),
        work: te.get_work().to_string(),
        movement: te.get_movement().to_string(),
        movement_number: te.get_movement_number().to_string(),
        movement_total: te.get_movement_total().to_string(),
        initial_key: te.get_initial_key().to_string(),
        mood: te.get_mood().to_string(),
        language: te.get_language().to_string(),
        isrc: te.get_isrc().to_string(),
        copyright: te.get_copyright().to_string(),
        label: te.get_label().to_string(),
        catalog_number: te.get_catalog_number().to_string(),
        barcode: te.get_barcode().to_string(),
        media: te.get_media().to_string(),
        release_type: te.get_release_type().to_string(),
        release_country: te.get_release_country().to_string(),
        lyrics: te.get_lyrics().to_string(),
    }
}

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
    /// The whole form **as populated into the Slint properties**, so the commit diff is a plain
    /// field-by-field compare against what the user was shown.
    originals: FormState,
    /// The four list fields as populated. Structural rather than rendered lines, for the reason
    /// [`ListFields`] gives.
    original_lists: ListFields,
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
    wire_genres(&te, ui);
    wire_roles(&te, ui);
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
                let read =
                    s.runtime.spawn_blocking(move || library::tags::read_lyrics_and_credits(&path));
                match read.await {
                    Ok(Ok((lyrics, credits))) => (lyrics.unwrap_or_default(), vec![credits]),
                    Ok(Err(e)) => {
                        log::warn!("tag edit: reading {}: {}", rows[0].file_path, describe(&e));
                        (String::new(), vec![<(ArtistCredit, ArtistCredit)>::default()])
                    }
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

            // Always from the database, single selection included — argued at
            // `library::tags::get_tag_edit_role_credits`.
            let roles_by_id =
                library::tags::get_tag_edit_role_credits(&s, &ids).await.unwrap_or_else(|e| {
                    log::warn!("tag edit role credits: {}", describe(&e));
                    HashMap::new()
                });
            let roles: Vec<RoleCredits> =
                rows.iter().map(|r| roles_by_id.get(&r.id).cloned().unwrap_or_default()).collect();

            let genres_by_id =
                library::tags::get_tag_edit_genres(&s, &ids).await.unwrap_or_else(|e| {
                    log::warn!("tag edit genres: {}", describe(&e));
                    HashMap::new()
                });
            let genre_lists: Vec<GenreList> =
                rows.iter().map(|r| genres_by_id.get(&r.id).cloned().unwrap_or_default()).collect();

            // Release-level tags live on `albums`, so the Details tab reads them through the album
            // each selected track sits on.
            let release =
                library::tags::get_tag_edit_release_tags(&s, &ids).await.unwrap_or_else(|e| {
                    log::warn!("tag edit release tags: {}", describe(&e));
                    Vec::new()
                });

            let Some(ui) = weak.upgrade() else { return };
            populate(
                &ui,
                &session,
                Fetched {
                    rows,
                    credits,
                    roles,
                    genre_lists,
                    release,
                    lyrics,
                    resident,
                    cover,
                },
            );
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
/// The genre list's three callbacks, mirroring [`wire_credits`] minus the join phrase.
///
/// A model of `SharedString` rather than a row struct: a genre is a name and nothing else, where
/// an `ArtistCreditRow` exists to carry the phrase beside it.
fn wire_genres(te: &TagEditor, ui: &AppWindow) {
    // Installed once and never replaced, for `wire_credits`' reason: a default-constructed
    // `ModelRc` silently ignores `set_vec`.
    te.set_genres(ModelRc::new(VecModel::from(vec![SharedString::default()])));

    let weak = ui.as_weak();
    te.on_add_genre(move || {
        let Some(ui) = weak.upgrade() else { return };
        with_genres_model(&ui, |vm| vm.push(SharedString::default()));
    });

    let weak = ui.as_weak();
    te.on_remove_genre(move |row| {
        let Some(ui) = weak.upgrade() else { return };
        with_genres_model(&ui, |vm| {
            if let Ok(row) = usize::try_from(row)
                && row < vm.row_count()
            {
                vm.remove(row);
            }
            // A field with no rows would save as "cleared", which is not what removing one genre
            // means — the same state `wire_credits` keeps its own editor out of.
            if vm.row_count() == 0 {
                vm.push(SharedString::default());
            }
        });
    });

    let weak = ui.as_weak();
    te.on_set_genre_name(move |row, name| {
        let Some(ui) = weak.upgrade() else { return };
        with_genres_model(&ui, |vm| {
            if let Ok(row) = usize::try_from(row)
                && row < vm.row_count()
            {
                vm.set_row_data(row, name);
            }
        });
    });
}

/// The role lists' three callbacks — [`wire_genres`] once more, indexed by role.
///
/// One outer model of ten inner ones, so a role is a position rather than a property name and the
/// ten editors are one mount. `ROLES` is that position's meaning on both sides.
fn wire_roles(te: &TagEditor, ui: &AppWindow) {
    let rows: Vec<ModelRc<SharedString>> =
        ROLES.iter().map(|_| ModelRc::new(VecModel::from(vec![SharedString::default()]))).collect();
    te.set_role_names(ModelRc::new(VecModel::from(rows)));

    let weak = ui.as_weak();
    te.on_add_role(move |role| {
        let Some(ui) = weak.upgrade() else { return };
        with_role_model(&ui, role, |vm| vm.push(SharedString::default()));
    });

    let weak = ui.as_weak();
    te.on_remove_role(move |role, row| {
        let Some(ui) = weak.upgrade() else { return };
        with_role_model(&ui, role, |vm| {
            if let Ok(row) = usize::try_from(row)
                && row < vm.row_count()
            {
                vm.remove(row);
            }
            if vm.row_count() == 0 {
                vm.push(SharedString::default());
            }
        });
    });

    let weak = ui.as_weak();
    te.on_set_role_name(move |role, row, name| {
        let Some(ui) = weak.upgrade() else { return };
        with_role_model(&ui, role, |vm| {
            if let Ok(row) = usize::try_from(row)
                && row < vm.row_count()
            {
                vm.set_row_data(row, name);
            }
        });
    });
}

/// Run `f` over one role's row model, if `role` names one and it is the model `wire_roles`
/// installed.
fn with_role_model(ui: &AppWindow, role: i32, f: impl FnOnce(&VecModel<SharedString>)) {
    let Ok(role) = usize::try_from(role) else {
        return;
    };
    let Some(inner) = ui.global::<TagEditor>().get_role_names().row_data(role) else {
        return;
    };
    if let Some(vm) = inner.as_any().downcast_ref::<VecModel<SharedString>>() {
        f(vm);
    }
}

/// Fill every role's rows, keeping one blank row per role so each editor has a field to type into.
fn write_role_rows(ui: &AppWindow, credits: &RoleCredits) {
    for (index, role) in ROLES.into_iter().enumerate() {
        let mut names: Vec<SharedString> =
            credits.for_role(role).map(|credit| SharedString::from(credit.name.as_str())).collect();
        if names.is_empty() {
            names.push(SharedString::default());
        }
        with_role_model(ui, i32::try_from(index).unwrap_or(i32::MAX), |vm| vm.set_vec(names));
    }
}

/// The role credit set the form currently shows, blanks dropped.
///
/// Rebuilt in `ROLES` order, which is also the order the rows are mounted in, so the rendered
/// `credits` line is stable across a save that changed one name.
fn roles_from_model(ui: &AppWindow, original: &RoleCredits) -> RoleCredits {
    let outer = ui.global::<TagEditor>().get_role_names();
    let mut credits = Vec::new();
    for (index, role) in ROLES.into_iter().enumerate() {
        let Some(inner) = outer.row_data(index) else {
            continue;
        };
        credits.extend(inner.iter().filter_map(|name| {
            let name = name.trim();
            (!name.is_empty()).then(|| RoleCredit {
                role,
                detail: detail_for(original, role, name),
                name: name.to_owned(),
            })
        }));
    }
    RoleCredits::new(credits)
}

/// The instrument or voice `name` was credited with in `role`, carried across from what the file
/// said.
///
/// The editor has no field for it — only [`CreditRole::Performer`] has one at all, and a second
/// column on every row would be ten empty boxes to explain one. Without this the round trip is
/// *lossy in both directions*: the read-back would differ from the baseline for any track carrying
/// one, so [`diff_roles`] would answer `Set` on a form nobody touched and write the credit back
/// with the instrument stripped. Matched on the name, which is the only handle a row still has.
fn detail_for(original: &RoleCredits, role: CreditRole, name: &str) -> String {
    original
        .for_role(role)
        .find(|credit| credit.name.eq_ignore_ascii_case(name))
        .map(|credit| credit.detail.clone())
        .unwrap_or_default()
}

/// Run `f` over the genre model, if it is the `VecModel` `wire_genres` installed.
fn with_genres_model(ui: &AppWindow, f: impl FnOnce(&VecModel<SharedString>)) {
    let model = ui.global::<TagEditor>().get_genres();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<SharedString>>() {
        f(vm);
    }
}

/// Fill the genre rows, keeping one blank row for an empty list so the editor always has a field
/// to type into.
fn write_genre_rows(te: &TagEditor, genres: &GenreList) {
    let mut rows: Vec<SharedString> =
        genres.names().iter().map(|name| SharedString::from(name.as_str())).collect();
    if rows.is_empty() {
        rows.push(SharedString::default());
    }
    let model = te.get_genres();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<SharedString>>() {
        vm.set_vec(rows);
    }
}

/// The genre list a selection agrees on, and whether it disagreed.
///
/// `common_credit`'s contract, structural rather than over a rendered string: comparing the lists
/// themselves is what lets a genre contain the separator its column happens to render with.
fn common_genres<'a>(lists: impl Iterator<Item = &'a GenreList>) -> (GenreList, bool) {
    let mut lists = lists;
    let Some(first) = lists.next() else {
        return (GenreList::default(), false);
    };
    if lists.all(|other| other == first) {
        (first.clone(), false)
    } else {
        (GenreList::default(), true)
    }
}

/// The genre list the form currently shows, blanks dropped.
fn genres_from_model(ui: &AppWindow) -> GenreList {
    let names = ui
        .global::<TagEditor>()
        .get_genres()
        .iter()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect();
    GenreList::new(names)
}

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
            let lists = ListFields {
                credits: [
                    credit_from_model(&ui, CREDIT_ARTIST, &sess.join_phrases),
                    credit_from_model(&ui, CREDIT_ALBUM_ARTIST, &sess.join_phrases),
                ],
                genres: genres_from_model(&ui),
                roles: roles_from_model(&ui, &sess.original_lists.roles),
            };
            let edit = build_edit(
                &read_form(&te),
                &sess.originals,
                &lists,
                &sess.original_lists,
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

/// Everything one open of the dialog fetched, before any of it reaches a property.
///
/// One value rather than seven parameters: the four slices are all per-selected-track and
/// **parallel to `rows`**, which is an invariant a reader can check here and could not check across
/// an argument list. The three singles come off the same fetch and travel with them.
struct Fetched {
    rows: Vec<TagEditRow>,
    credits: Vec<(ArtistCredit, ArtistCredit)>,
    roles: Vec<RoleCredits>,
    genre_lists: Vec<GenreList>,
    release: Vec<ReleaseTagRow>,
    lyrics: String,
    resident: Option<String>,
    cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
}

/// Fill the `TagEditor` global from the fetched rows and record the snapshot.
fn populate(ui: &AppWindow, session: &Rc<RefCell<TagSession>>, fetched: Fetched) {
    let Fetched {
        rows,
        credits,
        roles,
        genre_lists,
        release,
        lyrics,
        resident,
        cover,
    } = fetched;
    let (rows, credits, roles, release) =
        (rows.as_slice(), credits.as_slice(), roles.as_slice(), release.as_slice());

    let te = ui.global::<TagEditor>();
    let sentinel = ui.global::<Settings>().invoke_tag_multiple_values();

    let mut originals = FormState::default();

    // Set one field: value + placeholder (the ‹multiple values› sentinel iff the
    // selection disagrees) + record the populated string as the diff baseline.
    //
    // The leading field name is what pairs this with `read_text_fields` — the commit reads the
    // same name back, so the two can't drift the way an index into a shared array could.
    macro_rules! field {
        ($name:ident, $val:expr, $set:ident, $set_ph:ident) => {{
            let (value, disagrees) = $val;
            te.$set(SharedString::from(value.as_str()));
            te.$set_ph(if disagrees {
                sentinel.clone()
            } else {
                SharedString::default()
            });
            originals.text.$name = value;
        }};
    }

    /// A `TagEditRow` string column, folded to the value the whole selection agrees on.
    macro_rules! shared {
        ($field:ident) => {
            common_str(rows.iter().map(|r| r.$field.as_deref().unwrap_or_default()))
        };
    }

    field!(
        title,
        common_str(rows.iter().map(|r| r.title.as_str())),
        set_title,
        set_title_placeholder
    );

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
    field!(album, shared!(album), set_album, set_album_placeholder);
    // The genre list, one row per name. `common_genres` picks the sentinel the way `common_str`
    // does; the rows themselves are the baseline, so nothing is recorded in `originals.text`.
    let (genres, genres_disagree) = common_genres(genre_lists.iter());
    write_genre_rows(&te, &genres);
    te.set_genre_placeholder(if genres_disagree {
        sentinel.clone()
    } else {
        SharedString::default()
    });
    field!(
        year,
        common_by(rows.iter().map(|r| r.year), int_key, fmt_int),
        set_year,
        set_year_placeholder
    );
    field!(
        original_year,
        common_by(rows.iter().map(|r| r.original_year), int_key, fmt_int),
        set_original_year,
        set_original_year_placeholder
    );
    field!(
        track_number,
        common_by(rows.iter().map(|r| r.track_number), int_key, fmt_int),
        set_track_number,
        set_track_number_placeholder
    );
    field!(
        track_total,
        common_by(rows.iter().map(|r| r.track_total), int_key, fmt_int),
        set_track_total,
        set_track_total_placeholder
    );
    field!(
        disc_number,
        common_by(rows.iter().map(|r| r.disc_number), int_key, fmt_int),
        set_disc_number,
        set_disc_number_placeholder
    );
    field!(
        disc_total,
        common_by(rows.iter().map(|r| r.disc_total), int_key, fmt_int),
        set_disc_total,
        set_disc_total_placeholder
    );
    field!(comment, shared!(comment), set_comment, set_comment_placeholder);
    field!(
        bpm,
        common_by(rows.iter().map(|r| r.bpm), bpm_key, fmt_bpm),
        set_bpm,
        set_bpm_placeholder
    );

    // The role credits, a list of rows per role. Structural like the genres and the two artist
    // credits, so the sentinel is per role: one selection can agree on the composer and disagree
    // on the producer.
    let (common_roles, role_disagreements) = common_roles(roles);
    write_role_rows(ui, &common_roles);
    te.set_role_placeholders(ModelRc::new(VecModel::from(
        role_disagreements
            .iter()
            .map(|disagrees| {
                if *disagrees {
                    sentinel.clone()
                } else {
                    SharedString::default()
                }
            })
            .collect::<Vec<_>>(),
    )));

    field!(subtitle, shared!(subtitle), set_subtitle, set_subtitle_placeholder);
    field!(disc_subtitle, shared!(disc_subtitle), set_disc_subtitle, set_disc_subtitle_placeholder);
    field!(grouping, shared!(grouping), set_grouping, set_grouping_placeholder);
    field!(work, shared!(work), set_work, set_work_placeholder);
    field!(movement, shared!(movement), set_movement, set_movement_placeholder);
    field!(
        movement_number,
        common_by(rows.iter().map(|r| r.movement_number), int_key, fmt_int),
        set_movement_number,
        set_movement_number_placeholder
    );
    field!(
        movement_total,
        common_by(rows.iter().map(|r| r.movement_total), int_key, fmt_int),
        set_movement_total,
        set_movement_total_placeholder
    );
    field!(initial_key, shared!(initial_key), set_initial_key, set_initial_key_placeholder);
    field!(mood, shared!(mood), set_mood, set_mood_placeholder);
    field!(language, shared!(language), set_language, set_language_placeholder);
    field!(isrc, shared!(isrc), set_isrc, set_isrc_placeholder);
    field!(copyright, shared!(copyright), set_copyright, set_copyright_placeholder);

    // Release-level, read off the album the selection sits on rather than off `tracks`.
    field!(
        label,
        common_str(release.iter().map(|r| r.label.as_str())),
        set_label,
        set_label_placeholder
    );
    field!(
        catalog_number,
        common_str(release.iter().map(|r| r.catalog_number.as_str())),
        set_catalog_number,
        set_catalog_number_placeholder
    );
    field!(
        barcode,
        common_str(release.iter().map(|r| r.barcode.as_str())),
        set_barcode,
        set_barcode_placeholder
    );
    field!(
        media,
        common_str(release.iter().map(|r| r.media.as_str())),
        set_media,
        set_media_placeholder
    );
    field!(
        release_type,
        common_str(release.iter().map(|r| r.release_type.as_str())),
        set_release_type,
        set_release_type_placeholder
    );
    field!(
        release_country,
        common_str(release.iter().map(|r| r.release_country.as_str())),
        set_release_country,
        set_release_country_placeholder
    );

    // A switch has no ‹multiple values› state to show, so a selection that disagrees starts off
    // and an untouched switch stays `Keep` — nothing is written unless the user moves it.
    originals.compilation = release.iter().all(|r| r.is_compilation) && !release.is_empty();
    te.set_compilation(originals.compilation);

    // Lyrics (single selection only; multi mode leaves it "" ⇒ Keep).
    te.set_lyrics(SharedString::from(lyrics.as_str()));
    te.set_lyrics_placeholder(SharedString::default());
    originals.text.lyrics = lyrics;

    finalize_populate(
        &te,
        session,
        rows,
        Baseline {
            originals,
            lists: ListFields {
                credits: [artist, album_artist],
                genres,
                roles: common_roles,
            },
            join_phrases: phrases,
        },
        resident,
        cover,
    );
}

/// The role credits a selection agrees on, and which roles it disagreed about.
///
/// Per role rather than for the set as a whole: a batch that shares a composer and differs on the
/// producer should still show the composer, the way `common_str` shows every other field the
/// selection agrees on.
fn common_roles(sets: &[RoleCredits]) -> (RoleCredits, [bool; ROLES.len()]) {
    let mut agreed = Vec::new();
    let mut disagreements = [false; ROLES.len()];

    for (index, role) in ROLES.into_iter().enumerate() {
        let mut per_track = sets.iter().map(|set| set.for_role(role).cloned().collect::<Vec<_>>());
        let Some(first) = per_track.next() else {
            continue;
        };
        if per_track.all(|other| other == first) {
            agreed.extend(first);
        } else {
            disagreements[index] = true;
        }
    }

    (RoleCredits::new(agreed), disagreements)
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

/// Everything the commit diffs against, as it was the moment the dialog opened.
///
/// One value rather than four parameters because that is what the four are: a baseline. They are
/// written once by `populate` and read once by the commit, and `join_phrases` rides with them
/// because the picker list the dialog offers is part of what the user was shown.
struct Baseline {
    originals: FormState,
    lists: ListFields,
    join_phrases: Vec<(String, String)>,
}

/// Finish a `populate`: scalar flags, cover, Summary, and the session snapshot.
fn finalize_populate(
    te: &TagEditor,
    session: &Rc<RefCell<TagSession>>,
    rows: &[TagEditRow],
    baseline: Baseline,
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
        originals: baseline.originals,
        original_lists: baseline.lists,
        join_phrases: baseline.join_phrases,
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
    form: &FormState,
    orig: &FormState,
    lists: &ListFields,
    was_lists: &ListFields,
    artwork: ArtworkEdit,
) -> TagEdit {
    let (cur, was) = (&form.text, &orig.text);
    TagEdit {
        title: diff_str(&cur.title, &was.title),
        artist: diff_credit(&lists.credits[CREDIT_ARTIST], &was_lists.credits[CREDIT_ARTIST]),
        album_artist: diff_credit(
            &lists.credits[CREDIT_ALBUM_ARTIST],
            &was_lists.credits[CREDIT_ALBUM_ARTIST],
        ),
        album: diff_str(&cur.album, &was.album),
        genres: diff_genre_list(&lists.genres, &was_lists.genres),
        credits: diff_roles(&lists.roles, &was_lists.roles),
        year: diff_parsed::<u16>(&cur.year, &was.year),
        original_year: diff_parsed::<u16>(&cur.original_year, &was.original_year),
        track_number: diff_parsed::<u32>(&cur.track_number, &was.track_number),
        track_total: diff_parsed::<u32>(&cur.track_total, &was.track_total),
        disc_number: diff_parsed::<u32>(&cur.disc_number, &was.disc_number),
        disc_total: diff_parsed::<u32>(&cur.disc_total, &was.disc_total),
        disc_subtitle: diff_str(&cur.disc_subtitle, &was.disc_subtitle),
        subtitle: diff_str(&cur.subtitle, &was.subtitle),
        comment: diff_str(&cur.comment, &was.comment),
        bpm: diff_bpm(&cur.bpm, &was.bpm),
        initial_key: diff_str(&cur.initial_key, &was.initial_key),
        mood: diff_str(&cur.mood, &was.mood),
        grouping: diff_str(&cur.grouping, &was.grouping),
        work: diff_str(&cur.work, &was.work),
        movement: diff_str(&cur.movement, &was.movement),
        movement_number: diff_parsed::<u32>(&cur.movement_number, &was.movement_number),
        movement_total: diff_parsed::<u32>(&cur.movement_total, &was.movement_total),
        language: diff_str(&cur.language, &was.language),
        copyright: diff_str(&cur.copyright, &was.copyright),
        isrc: diff_str(&cur.isrc, &was.isrc),
        label: diff_str(&cur.label, &was.label),
        catalog_number: diff_str(&cur.catalog_number, &was.catalog_number),
        barcode: diff_str(&cur.barcode, &was.barcode),
        media: diff_str(&cur.media, &was.media),
        release_type: diff_str(&cur.release_type, &was.release_type),
        release_country: diff_str(&cur.release_country, &was.release_country),
        compilation: diff_flag(form.compilation, orig.compilation),
        lyrics: diff_str(&cur.lyrics, &was.lyrics),
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

/// Tri-state for the genre list, compared structurally.
///
/// Not through the rendered line, for [`diff_credit`]'s reason one field over: the rows are what
/// the user edited and the line is derived from them, so a reordering that renders the same string
/// is still an edit the file should receive.
fn diff_genre_list(cur: &GenreList, orig: &GenreList) -> FieldEdit<GenreList> {
    if cur == orig {
        FieldEdit::Keep
    } else if cur.is_empty() {
        FieldEdit::Clear
    } else {
        FieldEdit::Set(cur.clone())
    }
}

/// Tri-state for the whole role credit set, over the ten boxes the Credits tab mounts.
///
/// **All ten or nothing**, because `TagEdit::credits` writes all ten: a set built from the one box
/// that changed would clear every role beside it. So the answer is `Keep` until *some* box moves,
/// and then the set is rebuilt from every box at once. That also makes the emptied box work —
/// a role the user cleared is simply absent from the rebuild, and the writer removes its key.
///
/// Rebuilt in [`ROLES`] order, so a save that changed one name leaves the rendered `credits` line
/// where it was rather than reordering it.
fn diff_roles(cur: &RoleCredits, orig: &RoleCredits) -> FieldEdit<RoleCredits> {
    if cur == orig {
        FieldEdit::Keep
    } else if cur.is_empty() {
        FieldEdit::Clear
    } else {
        FieldEdit::Set(cur.clone())
    }
}

/// Tri-state for a switch. `Set(false)` and `Clear` mean the same thing to the writer, so an
/// un-ticked box is `Set(false)` and removes the tag.
fn diff_flag(cur: bool, orig: bool) -> FieldEdit<bool> {
    if cur == orig {
        FieldEdit::Keep
    } else {
        FieldEdit::Set(cur)
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
