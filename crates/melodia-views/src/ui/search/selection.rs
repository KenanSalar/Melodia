//! Songs-list row selection for the Search view: a thin adapter over
//! [`crate::ui::list_selection`], whose range walks the displayed model rows, so it follows the
//! compact/full toggle without reaching the rows "Show all" hides.

use slint::ComponentHandle;

use super::SearchUi;
use crate::ui::list_selection;
use melodia_ui::{AppWindow, Search, TrackListRow as UiTrackListRow};

/// Computes the new selection for a row click and applies it. UI thread.
pub fn handle_select_row(
    ui: &AppWindow,
    search_ui: &SearchUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<Search>();
    list_selection::handle_curated_click(
        &g,
        &search_ui.state().applied_selection,
        idx,
        id,
        shift,
        ctrl,
    );
}

/// Takes every displayed row into the selection.
pub fn select_all(ui: &AppWindow, search_ui: &SearchUi) {
    let g = ui.global::<Search>();
    list_selection::select_all_curated(&g, &search_ui.state().applied_selection);
}

/// Resets the selection (Escape, the row menu's "Clear selection", section-leave, and a new
/// query).
pub fn clear_selection(ui: &AppWindow, search_ui: &SearchUi) {
    let g = ui.global::<Search>();
    list_selection::clear_curated_selection(&g, &search_ui.state().applied_selection);
}

/// Re-stamps the selection onto freshly built rows before they are pushed, so swapping
/// compact and full, a re-sort or a new result keeps it. UI thread.
pub fn restamp_rows(g: &Search, rows: &mut [UiTrackListRow]) {
    list_selection::restamp_curated_rows(g, rows);
}
