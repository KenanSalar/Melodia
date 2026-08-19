//! Recently-Played row selection: modifier-aware clicks, clear, and the per-row `selected` flag
//! writer. Thin adapter over [`crate::ui::list_selection`], mirroring `favorites::selection` —
//! the two are the same three calls against their own global.

use slint::ComponentHandle;

use super::RecentlyPlayedUi;
use crate::ui::list_selection;
use crate::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

/// Compute the new selection set for a row click and apply it. Click semantics match
/// `favorites::handle_select_row`. Runs on the UI thread.
pub fn handle_select_row(
    ui: &AppWindow,
    rp_ui: &RecentlyPlayedUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<RecentlyPlayed>();
    list_selection::handle_curated_click(
        &g,
        &rp_ui.state().applied_selection,
        idx,
        id,
        shift,
        ctrl,
    );
}

/// Reset selection (called from the action-pill "Clear" button and section-leave).
pub fn clear_selection(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    let g = ui.global::<RecentlyPlayed>();
    list_selection::clear_curated_selection(&g, &rp_ui.state().applied_selection);
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list before it's pushed into the
/// Slint model. Invoked from `songs::apply_filtered_tracks`.
pub fn restamp_rows(g: &RecentlyPlayed, rows: &mut [UiTrackListRow]) {
    list_selection::restamp_curated_rows(g, rows);
}
