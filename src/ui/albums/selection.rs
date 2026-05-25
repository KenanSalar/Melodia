//! Album Detail row-selection — thin per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use slint::ComponentHandle;

use super::AlbumsUi;
use crate::ui::detail_selection::{self, SelectionRefs};
use crate::{AlbumDetail, AppWindow};

/// Borrow the Albums detail caches the generic selection logic mutates.
fn refs(albums_ui: &AlbumsUi) -> SelectionRefs<'_> {
    SelectionRefs {
        tracks: &albums_ui.detail.tracks,
        applied: &albums_ui.detail.applied_selection,
    }
}

/// Compute the new selection state for a detail-row click and apply it.
pub fn handle_select_row(
    ui: &AppWindow,
    albums_ui: &AlbumsUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<AlbumDetail>();
    detail_selection::handle_select_row(&g, &refs(albums_ui), idx, id, shift, ctrl);
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection(ui: &AppWindow, albums_ui: &AlbumsUi) {
    let g = ui.global::<AlbumDetail>();
    detail_selection::clear_selection(&g, &refs(albums_ui));
}

/// Re-stamp the `selected` flag on the rows whose selection membership
/// flipped. See [`detail_selection::apply_selection_to_rows`].
pub(super) fn apply_selection_to_rows(g: &AlbumDetail, albums_ui: &AlbumsUi) {
    detail_selection::apply_selection_to_rows(g, &refs(albums_ui));
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &AlbumDetail, ids: Vec<i32>) {
    detail_selection::write_selection(g, ids);
}
