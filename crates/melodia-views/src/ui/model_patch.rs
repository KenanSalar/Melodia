//! The favourite or rating patch every `VecModel` track list (Browse, Recently Played, Search)
//! writes into its changed rows; the per-view `apply_rows_*` wrappers keep only their hop and
//! global. A list over [`PagedRows`](crate::ui::paged_rows::PagedRows) patches its cache instead.

use std::collections::HashSet;
use std::hash::BuildHasher;

use slint::{Model, ModelRc, VecModel};

use crate::ui::util;
use melodia_ui::TrackListRow as UiTrackListRow;

#[cfg(test)]
#[path = "tests/model_patch_tests.rs"]
mod tests;

/// Apply `patch` to the rows whose id is in `ids`, writing those rows alone so the scroll and
/// their neighbours stay put, in one walk however many ids there are.
pub fn patch_track_rows_by_ids<S: BuildHasher>(
    rows: &ModelRc<UiTrackListRow>,
    ids: &HashSet<i64, S>,
    patch: impl Fn(&mut UiTrackListRow),
) {
    let Some(vm) = util::installed::<VecModel<UiTrackListRow>, _>(rows, "patch_track_rows_by_ids")
    else {
        return;
    };
    let mut unpatched = ids.len();
    for i in 0..vm.row_count() {
        // Ids are unique, and these models carry a whole filtered library rather than a
        // screenful, so every row past the last match would be a clone for nothing.
        if unpatched == 0 {
            return;
        }
        let Some(mut row) = vm.row_data(i) else {
            continue;
        };
        if !ids.contains(&i64::from(row.id)) {
            continue;
        }
        patch(&mut row);
        vm.set_row_data(i, row);
        unpatched -= 1;
    }
}
