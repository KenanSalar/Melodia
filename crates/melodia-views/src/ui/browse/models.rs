//! In-place writers for the three `Browse` `VecModel`s. Each falls back
//! to a fresh `ModelRc` only if the install step somehow didn't run.

use slint::{Model, ModelRc, VecModel};

use melodia_ui::{
    BreadcrumbRow as UiBreadcrumbRow, Browse, BrowseFolderRow as UiBrowseFolderRow,
    TrackListRow as UiTrackListRow,
};

pub(super) fn replace_folder_model(g: &Browse, rows: Vec<UiBrowseFolderRow>) {
    let model = g.get_folders();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiBrowseFolderRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_folders(ModelRc::new(VecModel::from(rows)));
    }
}

pub(super) fn replace_rows_model(g: &Browse, rows: Vec<UiTrackListRow>) {
    let model = g.get_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_rows(ModelRc::new(VecModel::from(rows)));
    }
}

pub(super) fn replace_breadcrumb_model(g: &Browse, rows: Vec<UiBreadcrumbRow>) {
    let model = g.get_breadcrumbs();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiBreadcrumbRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_breadcrumbs(ModelRc::new(VecModel::from(rows)));
    }
}
