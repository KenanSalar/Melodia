//! The Edit Artwork picker: the grid card's opener (the detail's is
//! `super::detail`'s, over the same [`open_edit_artwork`]), the mosaic-candidate
//! toggle, and the apply / clear commits the `Dialog.accepted` dispatcher routes.

use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel, Weak};

use super::refresh_after_edit;
use crate::ui::callbacks::DialogClaim;
use crate::ui::playlists::PlaylistsUi;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, Playlists};

/// How many covers a mosaic takes.
const MOSAIC_CAP: usize = 4;

pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    wire_request_edit_artwork_for(ui, state, playlists_ui);
    wire_toggle_mosaic_candidate(ui);
    wire_apply_mosaic(ui, state, playlists_ui);
    wire_clear_artwork(ui, state, playlists_ui);
}

/// Grid-card variant of the detail view's `request-edit-artwork`. The detail
/// view isn't necessarily open for this id, so the dialog preview's "current
/// artwork" is resolved against the saved `thumbnail_path` via the grid-tier
/// LRU (same path the card itself uses for `request-cover`, so the decode is
/// already warm). `slint::Image` isn't `Send`, so only the path crosses the
/// spawn boundary and the lookup runs inside the opener's UI-thread hop.
fn wire_request_edit_artwork_for(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_request_edit_artwork_for(move |id| {
        let id = i64::from(id);
        if id < 0 {
            return;
        }
        let Some(claim) = DialogClaim::take_from(&weak) else { return };
        let artwork_path =
            pu.grid_stats_by_id(id).and_then(|p| p.thumbnail_path).unwrap_or_default();
        open_edit_artwork(&s, &weak, claim, id, move |_| {
            if artwork_path.is_empty() {
                Image::default()
            } else {
                crate::ui::grid_prewarm::grid_cover_blocking(&artwork_path)
            }
        });
    });
}

/// Populate the mosaic candidates from playlist `id`'s own track artworks and open
/// the picker on `cover`, a preview of the saved state. The rest of the chrome is
/// `Dialog.prepare-edit-artwork`'s. `claim` is taken by the caller at its click.
pub(super) fn open_edit_artwork(
    state: &AppState,
    weak: &Weak<AppWindow>,
    claim: DialogClaim,
    id: i64,
    cover: impl FnOnce(&AppWindow) -> Image + Send + 'static,
) {
    let s = state.clone();
    let weak = weak.clone();
    state.runtime.spawn(async move {
        let candidates =
            library::playlists::get_playlist_artwork_paths(&s.db, id).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if !claim.holds(&ui) {
                return;
            }
            let current_cover = cover(&ui);
            let cand_rows: Vec<SharedString> =
                candidates.into_iter().map(SharedString::from).collect();
            let dlg = ui.global::<Dialog>();
            dlg.invoke_prepare_edit_artwork(i32::try_from(id).unwrap_or(-1));
            dlg.set_current_artwork(current_cover);
            dlg.set_mosaic_candidates(ModelRc::new(VecModel::from(cand_rows)));
            dlg.set_open(true);
        });
    });
}

/// Mutate `Dialog.mosaic-selection` on the UI thread. Any toggle —
/// including toggling the last entry back off — flips `mosaic-touched`, so
/// the preview switches off the "current saved artwork" branch and Apply can
/// wipe the artwork on an explicit clear.
fn wire_toggle_mosaic_candidate(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_toggle_mosaic_candidate(move |path| {
        let Some(ui) = weak.upgrade() else { return };
        let dlg = ui.global::<Dialog>();
        let current: Vec<SharedString> = dlg.get_mosaic_selection().iter().collect();
        let next = toggle_mosaic(current, &path);
        dlg.set_mosaic_selection(ModelRc::new(VecModel::from(next)));
        dlg.set_mosaic_touched(true);
    });
}

/// `path` toggled in or out of the selection. A path past [`MOSAIC_CAP`] is
/// silently refused.
fn toggle_mosaic(mut selection: Vec<SharedString>, path: &str) -> Vec<SharedString> {
    if let Some(pos) = selection.iter().position(|p| p.as_str() == path) {
        selection.remove(pos);
    } else if selection.len() < MOSAIC_CAP {
        selection.push(SharedString::from(path));
    }
    selection
}

/// The dispatcher hands us `(id, paths 1..4)`. Backend composes a 600x600
/// collage; we refresh.
fn wire_apply_mosaic(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_apply_mosaic(move |id, paths| {
        let id = i64::from(id);
        let path_vec: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        if path_vec.is_empty() {
            return;
        }
        let s = s.clone();
        let pu = pu.clone();
        let weak = weak.clone();
        s.runtime.clone().spawn(async move {
            if let Err(e) = library::playlists::set_playlist_thumbnail(&s, id, path_vec).await {
                log::warn!("playlists::apply_mosaic({id}): {}", describe(&e));
                return;
            }
            refresh_after_edit(&s, &pu, weak, &[id], "playlists::apply_mosaic").await;
        });
    });
}

/// An empty mosaic selection means "revert to auto" (clear the custom
/// thumbnail). Reuse `update_playlist` with `clear_thumbnail = true`;
/// description preserved.
fn wire_clear_artwork(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_clear_artwork(move |id| {
        let id = i64::from(id);
        let s = s.clone();
        let pu = pu.clone();
        let weak = weak.clone();
        s.runtime.clone().spawn(async move {
            let Ok(current) = library::playlists::get_playlist_detail(&s.db, id).await else {
                return;
            };
            if let Err(e) = library::playlists::update_playlist(
                &s.db,
                id,
                current.name,
                current.description,
                Some(true),
            )
            .await
            {
                log::warn!("playlists::clear_artwork: {}", describe(&e));
                return;
            }
            refresh_after_edit(&s, &pu, weak, &[id], "playlists::clear_artwork").await;
        });
    });
}
