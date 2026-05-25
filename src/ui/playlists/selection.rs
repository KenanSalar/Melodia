//! Playlist Detail row-selection — thin per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use slint::ComponentHandle;

use super::PlaylistsUi;
use crate::ui::detail_selection::{self, SelectionRefs};
use crate::{AppWindow, PlaylistDetail};

/// Borrow the Playlists detail caches the generic selection logic mutates.
fn refs(playlists_ui: &PlaylistsUi) -> SelectionRefs<'_> {
    SelectionRefs {
        tracks: &playlists_ui.detail.tracks,
        applied: &playlists_ui.detail.applied_selection,
    }
}

/// Compute the new selection state for a detail-row click and apply it.
pub fn handle_select_row(
    ui: &AppWindow,
    playlists_ui: &PlaylistsUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<PlaylistDetail>();
    detail_selection::handle_select_row(&g, &refs(playlists_ui), idx, id, shift, ctrl);
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection(ui: &AppWindow, playlists_ui: &PlaylistsUi) {
    let g = ui.global::<PlaylistDetail>();
    detail_selection::clear_selection(&g, &refs(playlists_ui));
}

/// Re-stamp the `selected` flag on the rows whose selection membership
/// flipped. See [`detail_selection::apply_selection_to_rows`].
pub(super) fn apply_selection_to_rows(g: &PlaylistDetail, playlists_ui: &PlaylistsUi) {
    detail_selection::apply_selection_to_rows(g, &refs(playlists_ui));
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &PlaylistDetail, ids: Vec<i32>) {
    detail_selection::write_selection(g, ids);
}
