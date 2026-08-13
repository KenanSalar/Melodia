//! Track-row selection: modifier-aware clicks, clear, and the persistent
//! `selected-ids` model writer. Thin adapter over
//! [`crate::ui::list_selection`].

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc};

use super::TracksUi;
use crate::ui::list_selection;
use crate::ui::util::clamp_i64_to_i32;
use crate::{AppWindow, Tracks};

/// Compute the new selection state for a row click and apply it: write back
/// `selected-ids` + `selection-anchor` on the global, then walk the visible
/// `VecModel<TrackListRow>` to flip per-row `selected` flags.
///
/// Runs on the UI thread (called from `on_select_row`); no
/// `upgrade_in_event_loop` indirection is needed.
pub fn handle_select_row(
    ui: &AppWindow,
    tracks_ui: &TracksUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<Tracks>();
    let cur_anchor = g.get_selection_anchor();
    let cur_selected: Vec<i32> = g.get_selected_ids().iter().collect();

    let (new_selected, new_anchor) = list_selection::compute_click_selection(
        cur_anchor,
        cur_selected,
        // Range select: needs ids in current filtered display order. Only
        // computed in the Shift branch — the filter walk is not free.
        || {
            let filter = g.get_filter().to_string();
            tracks_ui.current_ids_filtered(&filter).iter().map(|&v| clamp_i64_to_i32(v)).collect()
        },
        idx,
        id,
        shift,
        ctrl,
    );

    let id_set: HashSet<i32> = new_selected.iter().copied().collect();
    write_selection(&g, new_selected);
    g.set_selection_anchor(new_anchor);

    // Update per-row flags so the visual reflects the new selection.
    list_selection::stamp_rows_selected(&g.get_rows(), &id_set);
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection(ui: &AppWindow) {
    let g = ui.global::<Tracks>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(-1);
    list_selection::stamp_rows_selected(&g.get_rows(), &HashSet::new());
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &Tracks, ids: Vec<i32>) {
    list_selection::write_selection_ids(&g.get_selected_ids(), ids, |m: ModelRc<i32>| {
        g.set_selected_ids(m);
    });
}
