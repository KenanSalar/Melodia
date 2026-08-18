//! Favorites All-Songs row selection: modifier-aware clicks, clear, and the per-row `selected`
//! flag writer that drives row highlight + checkbox state.
//!
//! Thin adapter over [`crate::ui::list_selection`] — the `TrackList` component reads each row's
//! `selected: bool` for its checkbox tick and accent-tinted background, so a click that only
//! updates `selected-ids` (without re-stamping the per-row flag) leaves the row visually
//! un-selected even though it counts toward the "{n} selected" chip.

use slint::ComponentHandle;

use super::FavoritesUi;
use crate::ui::list_selection;
use crate::{AppWindow, Favorites, TrackListRow as UiTrackListRow};

/// Compute the new selection set for a row click and apply it. Click semantics match
/// `tracks::handle_select_row` exactly. Runs on the UI thread (called from `on_select_row`).
pub fn handle_select_row(
    ui: &AppWindow,
    fav_ui: &FavoritesUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<Favorites>();
    list_selection::handle_curated_click(
        &g,
        &fav_ui.state().applied_selection,
        idx,
        id,
        shift,
        ctrl,
    );
}

/// Reset selection (called from the action-pill "Clear" button and section-leave).
pub fn clear_selection(ui: &AppWindow, fav_ui: &FavoritesUi) {
    let g = ui.global::<Favorites>();
    list_selection::clear_curated_selection(&g, &fav_ui.state().applied_selection);
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list before it's pushed into the
/// Slint model. Invoked from `songs::apply_filtered_tracks` so a filter change / library refresh
/// doesn't drop the user's existing selection.
pub fn restamp_rows(g: &Favorites, rows: &mut [UiTrackListRow]) {
    list_selection::restamp_curated_rows(g, rows);
}
