//! Row-selection: modifier-aware click handler, clear, and the persistent
//! `selected-ids` model writer.

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::BrowseUi;
use crate::ui::list_selection::{NO_ANCHOR, compute_click_selection};
use crate::ui::util::clamp_i64_to_i32;
use melodia_core::entities::browse::BrowseFile;
use melodia_ui::{AppWindow, Browse};

/// Compute the new selection state for a row click and apply it, through
/// `list_selection`'s click semantics. Disk-only rows all share `id == 0`,
/// which an id-keyed selection cannot hold, so a click on one is ignored.
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
    let (new_selected, new_anchor) = compute_click_selection(
        g.get_selection_anchor(),
        g.get_selected_ids().iter().collect(),
        || row_ids(&browse_ui.last_files.lock()),
        idx,
        id,
        shift,
        ctrl,
    );

    write_selection(&g, new_selected);
    g.set_selection_anchor(new_anchor);
    apply_selection_to_rows(&g);
}

/// Every displayed row's id, a disk-only row's as `0`, so a click's index still names its row.
fn row_ids(files: &[BrowseFile]) -> Vec<i32> {
    files.iter().map(|f| if f.in_library { clamp_i64_to_i32(f.row.id) } else { 0 }).collect()
}

/// Take every displayed row into the selection. Disk-only rows drop out inside
/// `select_all_ids`, which filters on the same `enabled` / `id != 0` pair the click handler
/// guards on, and stamps them unselected in the walk that collects the rest.
pub fn select_all(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    let ids = crate::ui::list_selection::select_all_ids(&g.get_rows());
    write_selection(&g, ids);
}

/// Reset selection (Escape, the row menu's "Clear selection", and section-leave).
pub fn clear_selection(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(NO_ANCHOR);
    crate::ui::list_selection::stamp_rows_selected(&g.get_rows(), &HashSet::new());
}

/// Clear the persistent `selected-ids` model + anchor without walking
/// the row model. Used on every fetch — the freshly-built rows already
/// carry `selected: false`, so there's nothing per-row to undo.
pub(super) fn reset_selection(g: &Browse) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(NO_ANCHOR);
}

/// Flip each row's `selected` flag to match the current `selected-ids` set, this view's
/// selection living on the global rather than being handed in.
pub(super) fn apply_selection_to_rows(g: &Browse) {
    let selected: HashSet<i32> = g.get_selected_ids().iter().collect();
    crate::ui::list_selection::stamp_rows_selected(&g.get_rows(), &selected);
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place. Falls
/// back to a fresh `ModelRc` only if the install step somehow didn't run.
///
/// Every selection write lands here, which is why the card grid's generation
/// is bumped here rather than at the three callers.
pub(super) fn write_selection(g: &Browse, ids: Vec<i32>) {
    let model = g.get_selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(ids);
    } else {
        g.set_selected_ids(ModelRc::new(VecModel::from(ids)));
    }
    g.set_selection_generation(g.get_selection_generation().wrapping_add(1));
}

#[cfg(test)]
#[path = "tests/selection_tests.rs"]
mod tests;
