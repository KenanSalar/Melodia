//! Track-row selection: modifier-aware clicks, clear, and the persistent
//! `selected-ids` model writer.

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::TracksUi;
use crate::ui::util::clamp_i64_to_i32;
use crate::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

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

    let (new_selected, new_anchor) = if shift && cur_anchor >= 0 {
        // Range select: need ids in current filtered display order.
        let filter = g.get_filter().to_string();
        let filtered = tracks_ui.current_ids_filtered(&filter);
        if filtered.is_empty() {
            (vec![id], idx)
        } else {
            let filtered_i32: Vec<i32> =
                filtered.iter().map(|&v| clamp_i64_to_i32(v)).collect();
            let last = i32::try_from(filtered_i32.len() - 1).unwrap_or(i32::MAX);
            let lo = usize::try_from(cur_anchor.min(idx).clamp(0, last)).unwrap_or(0);
            let hi = usize::try_from(cur_anchor.max(idx).clamp(0, last)).unwrap_or(0);
            (filtered_i32[lo..=hi].to_vec(), cur_anchor)
        }
    } else if ctrl {
        let mut next = cur_selected.clone();
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

    // Update per-row flags so the visual reflects the new selection.
    let rows = g.get_rows();
    if let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        for i in 0..vm.row_count() {
            let Some(mut r) = vm.row_data(i) else { continue };
            let now = id_set.contains(&r.id);
            if r.selected != now {
                r.selected = now;
                vm.set_row_data(i, r);
            }
        }
    }
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection(ui: &AppWindow) {
    let g = ui.global::<Tracks>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(-1);
    let rows = g.get_rows();
    if let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        for i in 0..vm.row_count() {
            let Some(mut r) = vm.row_data(i) else { continue };
            if r.selected {
                r.selected = false;
                vm.set_row_data(i, r);
            }
        }
    }
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place. Falls back
/// to a fresh `ModelRc` only if the install step somehow didn't run.
pub(super) fn write_selection(g: &Tracks, ids: Vec<i32>) {
    let model = g.get_selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(ids);
    } else {
        g.set_selected_ids(ModelRc::new(VecModel::from(ids)));
    }
}
