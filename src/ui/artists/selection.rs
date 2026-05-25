//! Artist Detail row-selection — thin per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use slint::ComponentHandle;

use super::ArtistsUi;
use crate::ui::detail_selection::{self, SelectionRefs};
use crate::{AppWindow, ArtistDetail};

/// Borrow the Artists detail caches the generic selection logic mutates.
fn refs(artists_ui: &ArtistsUi) -> SelectionRefs<'_> {
    SelectionRefs {
        tracks: &artists_ui.detail.tracks,
        applied: &artists_ui.detail.applied_selection,
    }
}

/// Compute the new selection state for a detail-row click and apply it.
pub fn handle_select_row(
    ui: &AppWindow,
    artists_ui: &ArtistsUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<ArtistDetail>();
    detail_selection::handle_select_row(&g, &refs(artists_ui), idx, id, shift, ctrl);
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection(ui: &AppWindow, artists_ui: &ArtistsUi) {
    let g = ui.global::<ArtistDetail>();
    detail_selection::clear_selection(&g, &refs(artists_ui));
}

/// Re-stamp the `selected` flag on the rows whose selection membership
/// flipped. See [`detail_selection::apply_selection_to_rows`].
pub(super) fn apply_selection_to_rows(g: &ArtistDetail, artists_ui: &ArtistsUi) {
    detail_selection::apply_selection_to_rows(g, &refs(artists_ui));
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &ArtistDetail, ids: Vec<i32>) {
    detail_selection::write_selection(g, ids);
}
