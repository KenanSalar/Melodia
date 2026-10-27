//! The file list's row selection, kept in the persistent `Browse.selected-ids`.

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::BrowseUi;
use crate::ui::list_selection::{NO_ANCHOR, compute_click_selection};
use crate::ui::util::clamp_i64_to_i32;
use melodia_core::entities::browse::BrowseFile;
use melodia_ui::{AppWindow, Browse};

/// Applies a row click through `list_selection`'s semantics. Disk-only rows all share `id == 0`,
/// which an id-keyed selection can't hold, so a click on one is ignored.
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

/// Selects every displayed row; `select_all_ids` drops the disk-only ones on the click handler's
/// own guard.
pub fn select_all(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    let ids = crate::ui::list_selection::select_all_ids(&g.get_rows());
    write_selection(&g, ids);
}

pub fn clear_selection(ui: &AppWindow) {
    let g = ui.global::<Browse>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(NO_ANCHOR);
    crate::ui::list_selection::stamp_rows_selected(&g.get_rows(), &HashSet::new());
}

/// [`clear_selection`] without the row walk, for a fetch whose fresh rows are all unselected.
pub(super) fn reset_selection(g: &Browse) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(NO_ANCHOR);
}

/// Stamps each row's `selected` from `Browse.selected-ids`.
pub(super) fn apply_selection_to_rows(g: &Browse) {
    let selected: HashSet<i32> = g.get_selected_ids().iter().collect();
    crate::ui::list_selection::stamp_rows_selected(&g.get_rows(), &selected);
}

/// Every selection write lands here, which is why the card grid's generation is bumped here
/// rather than at the callers.
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
