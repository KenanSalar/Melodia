//! Row-selection: modifier-aware click handler, clear, and the persistent
//! `selected-ids` model writer.

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::BrowseUi;
use crate::ui::util::clamp_i64_to_i32;
use melodia_ui::{AppWindow, Browse, TrackListRow as UiTrackListRow};

/// Compute the new selection state for a row click and apply it. Mirrors
/// `tracks::handle_select_row`, with one Browse-specific guard: disk-only
/// rows have `id == 0` (which would collide in an id-keyed selection
/// model), so they're never selectable — the handler returns early on
/// `id == 0` and filters `0`s out of any shift-range result.
///
/// Runs on the UI thread (called from `on_select_row`).
pub fn handle_select_row(
    ui: &AppWindow,
    browse_ui: &BrowseUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    if id == 0 {
        return;
    }
    let g = ui.global::<Browse>();
    let cur_anchor = g.get_selection_anchor();
    let cur_selected: Vec<i32> = g.get_selected_ids().iter().collect();

    let (new_selected, new_anchor) = if shift && cur_anchor >= 0 {
        // Range select over the displayed rows (`last_files` order).
        let files = browse_ui.last_files.lock();
        if files.is_empty() {
            (vec![id], idx)
        } else {
            let last = i32::try_from(files.len().saturating_sub(1)).unwrap_or(i32::MAX);
            let lo = usize::try_from(cur_anchor.min(idx).clamp(0, last)).unwrap_or(0);
            let hi = usize::try_from(cur_anchor.max(idx).clamp(0, last)).unwrap_or(0);
            // Disk-only rows in the span are skipped — they aren't
            // selectable and all share `id == 0`.
            let range: Vec<i32> = files[lo..=hi]
                .iter()
                .filter_map(|f| {
                    let rid = clamp_i64_to_i32(f.row.id);
                    (f.in_library && rid != 0).then_some(rid)
                })
                .collect();
            if range.is_empty() {
                (vec![id], idx)
            } else {
                (range, cur_anchor)
            }
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

    write_selection(&g, new_selected);
    g.set_selection_anchor(new_anchor);
    apply_selection_to_rows(&g);
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(-1);
    let rows = g.get_rows();
    if let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        for i in 0..vm.row_count() {
            let Some(mut r) = vm.row_data(i) else {
                continue;
            };
            if r.selected {
                r.selected = false;
                vm.set_row_data(i, r);
            }
        }
    }
}

/// Clear the persistent `selected-ids` model + anchor without walking
/// the row model. Used on every fetch — the freshly-built rows already
/// carry `selected: false`, so there's nothing per-row to undo.
pub(super) fn reset_selection(g: &Browse) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(-1);
}

/// Walk the `Browse.rows` `VecModel` and flip each row's `selected` flag
/// to match the current `selected-ids` set.
pub(super) fn apply_selection_to_rows(g: &Browse) {
    let selected: HashSet<i32> = g.get_selected_ids().iter().collect();
    let rows = g.get_rows();
    if let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        for i in 0..vm.row_count() {
            let Some(mut r) = vm.row_data(i) else {
                continue;
            };
            let now = selected.contains(&r.id);
            if r.selected != now {
                r.selected = now;
                vm.set_row_data(i, r);
            }
        }
    }
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place. Falls
/// back to a fresh `ModelRc` only if the install step somehow didn't run.
pub(super) fn write_selection(g: &Browse, ids: Vec<i32>) {
    let model = g.get_selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(ids);
    } else {
        g.set_selected_ids(ModelRc::new(VecModel::from(ids)));
    }
}
