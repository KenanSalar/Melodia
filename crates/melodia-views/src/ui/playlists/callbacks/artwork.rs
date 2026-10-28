//! The Edit Artwork picker: the card's opener (the detail's is `super::detail`'s, over the same
//! [`open_edit_artwork`]), the mosaic toggle, and the commits `Dialog.accepted` routes here.

use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel, Weak};

use super::refresh_after_edit;
use crate::ui::callbacks::DialogClaim;
use crate::ui::playlists::PlaylistsUi;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::smart_criteria::Draw;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, PlaylistRow as UiPlaylistRow, Playlists};

/// How many covers a mosaic takes.
const MOSAIC_CAP: usize = 4;

pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    wire_request_edit_artwork_for(ui, state);
    wire_toggle_mosaic_candidate(ui);
    wire_apply_mosaic(ui, state, playlists_ui);
    wire_clear_artwork(ui, state, playlists_ui);
}

/// The card's opener, which may have no detail open for this id, so the preview is the card's own
/// cover off the grid tier, already warm. `slint::Image` isn't `Send`, so only the path crosses
/// the spawn and the lookup runs in the opener's UI-thread hop.
fn wire_request_edit_artwork_for(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Playlists>().on_request_edit_artwork_for(move |id| {
        let id = i64::from(id);
        if id < 0 {
            return;
        }
        let Some(claim) = DialogClaim::take_from(&weak) else { return };
        let Some(ui) = weak.upgrade() else { return };
        // A miss is a card whose row hasn't landed: nothing to preview, and no kind to pick the
        // empty pool's hint by.
        let Some(card) = crate::ui::playlists::shown_card(&ui, id) else { return };
        let artwork_path = card.artwork_path.to_string();
        // The default draw, the one the card's own cover was filled under.
        open_edit_artwork(&s, &weak, claim, &card, Draw::default(), move |_| {
            if artwork_path.is_empty() {
                Image::default()
            } else {
                crate::ui::grid_prewarm::grid_cover_blocking(&artwork_path)
            }
        });
    });
}

/// Fills the mosaic candidates from the covers `playlist` lists under `draw` and opens the picker
/// on `cover`, the saved state. Only a random cap reads the draw: the page's on the detail, the
/// default a card's thumbnail was filled under. `claim` is the caller's, taken at its click.
pub(super) fn open_edit_artwork(
    state: &AppState,
    weak: &Weak<AppWindow>,
    claim: DialogClaim,
    playlist: &UiPlaylistRow,
    draw: Draw,
    cover: impl FnOnce(&AppWindow) -> Image + Send + 'static,
) {
    let (id, is_smart) = (playlist.id, playlist.is_smart);
    let scope = TrackListScope::Playlist(i64::from(id), draw);
    let s = state.clone();
    let weak = weak.clone();
    state.runtime.spawn(async move {
        let candidates =
            library::playlists::mosaic_candidates(&s.db, scope).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if !claim.holds(&ui) {
                return;
            }
            let current_cover = cover(&ui);
            let cand_rows: Vec<SharedString> =
                candidates.into_iter().map(SharedString::from).collect();
            let dlg = ui.global::<Dialog>();
            dlg.invoke_prepare_edit_artwork(id, is_smart);
            dlg.set_current_artwork(current_cover);
            dlg.set_mosaic_candidates(ModelRc::new(VecModel::from(cand_rows)));
            dlg.set_open(true);
        });
    });
}

/// Any toggle, the last entry's back off included, raises `mosaic-touched`, so the preview leaves
/// the saved artwork and Apply can clear it.
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

/// `path` toggled in or out of the selection, a path past [`MOSAIC_CAP`] refused.
fn toggle_mosaic(mut selection: Vec<SharedString>, path: &str) -> Vec<SharedString> {
    if let Some(pos) = selection.iter().position(|p| p.as_str() == path) {
        selection.remove(pos);
    } else if selection.len() < MOSAIC_CAP {
        selection.push(SharedString::from(path));
    }
    selection
}

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

/// An empty mosaic selection leaves the playlist with no artwork, an automatic cover included.
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
            let current = match library::playlists::get_playlist_detail(&s.db, id).await {
                Ok(current) => current,
                Err(e) => {
                    log::warn!("playlists::clear_artwork({id}) fetch: {}", describe(&e));
                    return;
                }
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
                log::warn!("playlists::clear_artwork({id}): {}", describe(&e));
                return;
            }
            refresh_after_edit(&s, &pu, weak, &[id], "playlists::clear_artwork").await;
        });
    });
}
