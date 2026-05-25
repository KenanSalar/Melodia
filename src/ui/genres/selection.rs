//! Genre Detail row-selection — thin per-view adapter over the generic
//! [`crate::ui::detail_selection`] logic.

use slint::ComponentHandle;

use super::GenresUi;
use crate::ui::detail_selection::{self, SelectionRefs};
use crate::{AppWindow, GenreDetail};

/// Borrow the Genres detail caches the generic selection logic mutates.
fn refs(genres_ui: &GenresUi) -> SelectionRefs<'_> {
    SelectionRefs {
        tracks: &genres_ui.detail.tracks,
        applied: &genres_ui.detail.applied_selection,
    }
}

/// Compute the new selection state for a detail-row click and apply it.
pub fn handle_select_row(
    ui: &AppWindow,
    genres_ui: &GenresUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<GenreDetail>();
    detail_selection::handle_select_row(&g, &refs(genres_ui), idx, id, shift, ctrl);
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection(ui: &AppWindow, genres_ui: &GenresUi) {
    let g = ui.global::<GenreDetail>();
    detail_selection::clear_selection(&g, &refs(genres_ui));
}

/// Re-stamp the `selected` flag on the rows whose selection membership
/// flipped. See [`detail_selection::apply_selection_to_rows`].
pub(super) fn apply_selection_to_rows(g: &GenreDetail, genres_ui: &GenresUi) {
    detail_selection::apply_selection_to_rows(g, &refs(genres_ui));
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &GenreDetail, ids: Vec<i32>) {
    detail_selection::write_selection(g, ids);
}
