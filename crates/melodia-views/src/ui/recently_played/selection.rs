//! Recently-Played row selection: modifier-aware clicks, clear, and the per-row `selected` flag
//! writer. Thin adapter over [`crate::ui::list_selection`], mirroring `favorites::selection` —
//! the two are the same three calls against their own global.

use slint::ComponentHandle;

use crate::ui::list_selection;
use melodia_ui::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

/// Compute the new selection set for a row click and apply it. Click semantics match
/// `favorites::handle_select_row`. Runs on the UI thread.
pub fn handle_select_row(ui: &AppWindow, idx: i32, id: i32, shift: bool, ctrl: bool) {
    list_selection::handle_curated_click(&ui.global::<RecentlyPlayed>(), idx, id, shift, ctrl);
}

/// Take every displayed row into the selection.
pub fn select_all(ui: &AppWindow) {
    list_selection::select_all_curated(&ui.global::<RecentlyPlayed>());
}

/// Reset selection (Escape, the row menu's "Clear selection", and section-leave).
pub fn clear_selection(ui: &AppWindow) {
    list_selection::clear_curated_selection(&ui.global::<RecentlyPlayed>());
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list before it's pushed into the
/// Slint model. Invoked from `songs::apply_filtered_tracks`.
pub fn restamp_rows(g: &RecentlyPlayed, rows: &mut [UiTrackListRow]) {
    list_selection::restamp_curated_rows(g, rows);
}
