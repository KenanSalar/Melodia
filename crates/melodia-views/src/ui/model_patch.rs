//! The favourite or rating patch every `VecModel` track list (Browse, Recently Played, Search)
//! writes into its one changed row; the per-view `apply_row_*` wrappers keep only their hop and
//! global. A list over [`PagedRows`](crate::ui::paged_rows::PagedRows) patches its cache instead.

use slint::{Model, ModelRc, VecModel};

use crate::ui::util;
use melodia_ui::TrackListRow as UiTrackListRow;

#[cfg(test)]
#[path = "tests/model_patch_tests.rs"]
mod tests;

/// Apply `patch` to the row with `id`, where there is one, writing that row alone so the scroll
/// and its neighbours stay put.
pub fn patch_track_row_by_id(
    rows: &ModelRc<UiTrackListRow>,
    id: i64,
    patch: impl Fn(&mut UiTrackListRow),
) {
    let Some(vm) = util::installed::<VecModel<UiTrackListRow>, _>(rows, "patch_track_row_by_id")
    else {
        return;
    };
    for i in 0..vm.row_count() {
        let Some(mut row) = vm.row_data(i) else {
            continue;
        };
        if i64::from(row.id) != id {
            continue;
        }
        patch(&mut row);
        vm.set_row_data(i, row);
        // Ids are unique, and these models carry a whole filtered library rather than a
        // screenful, so every row past this one would be a clone for nothing.
        return;
    }
}
