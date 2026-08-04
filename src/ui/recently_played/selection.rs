//! Recently-Played row selection: modifier-aware clicks, clear, and the
//! per-row `selected` flag writer. Thin adapter over
//! [`crate::ui::list_selection`], mirroring `favorites::selection`.

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc};

use super::RecentlyPlayedUi;
use crate::ui::list_selection;
use crate::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

/// Compute the new selection set for a row click and apply it. Click semantics
/// match `favorites::handle_select_row`. Runs on the UI thread.
pub fn handle_select_row(
    ui: &AppWindow,
    rp_ui: &RecentlyPlayedUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<RecentlyPlayed>();
    let cur_anchor = g.get_selection_anchor();
    let cur_selected: Vec<i32> = g.get_selected_ids().iter().collect();

    let (new_selected, new_anchor) = list_selection::compute_click_selection(
        cur_anchor,
        cur_selected,
        // Range select over the currently-displayed rows (post-filter,
        // already in the same order as the Slint `tracks` model).
        || {
            let rows = g.get_tracks();
            (0..rows.row_count())
                .filter_map(|i| rows.row_data(i).map(|r| r.id))
                .collect()
        },
        idx,
        id,
        shift,
        ctrl,
    );

    let id_set: HashSet<i32> = new_selected.iter().copied().collect();
    write_selection(&g, new_selected);
    g.set_selection_anchor(new_anchor);
    (*rp_ui.state().applied_selection.lock()).clone_from(&id_set);

    list_selection::stamp_rows_selected(&g.get_tracks(), &id_set);
}

/// Reset selection (called from the action-pill "Clear" button and
/// section-leave).
pub fn clear_selection(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    let g = ui.global::<RecentlyPlayed>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(-1);
    rp_ui.state().applied_selection.lock().clear();
    list_selection::stamp_rows_selected(&g.get_tracks(), &HashSet::new());
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &RecentlyPlayed, ids: Vec<i32>) {
    list_selection::write_selection_ids(&g.get_selected_ids(), ids, |m: ModelRc<i32>| {
        g.set_selected_ids(m);
    });
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list before it's
/// pushed into the Slint model. Invoked from `tracks::apply_filtered_tracks`.
pub fn restamp_rows(g: &RecentlyPlayed, rows: &mut [UiTrackListRow]) {
    let selected_set: HashSet<i32> = g.get_selected_ids().iter().collect();
    list_selection::restamp_selected(rows, &selected_set);
}
