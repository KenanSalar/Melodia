//! Recently-Played row selection: modifier-aware clicks, clear, and the
//! per-row `selected` flag writer. Mirrors `favorites::selection` exactly —
//! plain click = single, Ctrl = toggle, Shift = range over the displayed
//! order.

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::RecentlyPlayedUi;
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

    let (new_selected, new_anchor) = if shift && cur_anchor >= 0 {
        // Range select over the currently-displayed rows (post-filter/sort,
        // already in the same order as the Slint `tracks` model).
        let rows = g.get_tracks();
        let row_count = rows.row_count();
        if row_count == 0 {
            (vec![id], idx)
        } else {
            let visible_ids: Vec<i32> = (0..row_count)
                .filter_map(|i| rows.row_data(i).map(|r| r.id))
                .collect();
            let last = i32::try_from(visible_ids.len().saturating_sub(1)).unwrap_or(i32::MAX);
            let lo = usize::try_from(cur_anchor.min(idx).clamp(0, last)).unwrap_or(0);
            let hi = usize::try_from(cur_anchor.max(idx).clamp(0, last)).unwrap_or(0);
            (visible_ids[lo..=hi].to_vec(), cur_anchor)
        }
    } else if ctrl {
        let mut next = cur_selected;
        if let Some(pos) = next.iter().position(|&v| v == id) {
            next.remove(pos);
        } else {
            next.push(id);
        }
        (next, idx)
    } else {
        (vec![id], idx)
    };

    let id_set: HashSet<i32> = new_selected.iter().copied().collect();
    write_selection(&g, new_selected);
    g.set_selection_anchor(new_anchor);
    (*rp_ui.state().applied_selection.lock()).clone_from(&id_set);

    apply_per_row_selection(&g, &id_set);
}

/// Reset selection (called from the action-pill "Clear" button and
/// section-leave).
pub fn clear_selection(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    let g = ui.global::<RecentlyPlayed>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(-1);
    rp_ui.state().applied_selection.lock().clear();
    apply_per_row_selection(&g, &HashSet::new());
}

/// Re-stamp `selected: bool` on every row to match `desired`. Rows whose flag
/// already matches are skipped so the `ListView` delegate cache survives.
fn apply_per_row_selection(g: &RecentlyPlayed, desired: &HashSet<i32>) {
    let rows = g.get_tracks();
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        return;
    };
    for i in 0..vm.row_count() {
        let Some(mut r) = vm.row_data(i) else { continue };
        let now = desired.contains(&r.id);
        if r.selected != now {
            r.selected = now;
            vm.set_row_data(i, r);
        }
    }
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &RecentlyPlayed, ids: Vec<i32>) {
    let model = g.get_selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(ids);
    } else {
        g.set_selected_ids(ModelRc::new(VecModel::from(ids)));
    }
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list before it's
/// pushed into the Slint model. Invoked from `tracks::apply_filtered_tracks`.
pub fn restamp_rows(g: &RecentlyPlayed, rows: &mut [UiTrackListRow]) {
    let selected_set: HashSet<i32> = g.get_selected_ids().iter().collect();
    if selected_set.is_empty() {
        return;
    }
    for row in rows {
        if selected_set.contains(&row.id) {
            row.selected = true;
        }
    }
}
