//! Surgical in-place patches on a Slint row `VecModel`.
//!
//! Every list surface (Tracks, Browse, the four detail views, Favorites,
//! Recently Played, Search) patches a favorite/rating change into its visible rows
//! the same way, and the queue sheet patches two of its own: walk the model, mutate
//! the rows that matter, write back only those. This module owns that walk; the
//! per-view `apply_row_*` wrappers keep only their `upgrade_in_event_loop`
//! hop and the global they read.

use slint::{Model, ModelRc, VecModel};

use melodia_ui::TrackListRow as UiTrackListRow;

#[cfg(test)]
#[path = "tests/model_patch_tests.rs"]
mod tests;

/// The `VecModel` behind `rows`, logged under `label` rather than passed over when it isn't one:
/// the rows and whatever the caller has in hand disagree from there on, and nothing else says so.
fn vec_model<'a, T: Clone + 'static>(rows: &'a ModelRc<T>, label: &str) -> Option<&'a VecModel<T>> {
    let found = rows.as_any().downcast_ref::<VecModel<T>>();
    if found.is_none() {
        log::warn!("{label}: VecModel downcast failed, rows left as they were");
    }
    found
}

/// Hand every row to `patch` as a clone, writing back the ones it reports as changed.
///
/// **The `bool` is what the walk is for.** `set_row_data` on an instantiated row re-runs that
/// delegate's bindings, so writing back a row nothing moved repaints it for no change; rows
/// outside the `ListView`'s window cost only the clone. UI-thread only.
pub fn patch_rows_where<T: Clone + 'static>(
    rows: &ModelRc<T>,
    label: &str,
    mut patch: impl FnMut(&mut T) -> bool,
) {
    let Some(vm) = vec_model(rows, label) else {
        return;
    };
    for i in 0..vm.row_count() {
        let Some(mut row) = vm.row_data(i) else {
            continue;
        };
        if patch(&mut row) {
            vm.set_row_data(i, row);
        }
    }
}

/// Find the row with `id` and apply `patch` to it. No-ops when the id isn't
/// present. Only the affected row is written, so scroll position and
/// neighbouring rows stay put.
pub fn patch_track_row_by_id(
    rows: &ModelRc<UiTrackListRow>,
    id: i64,
    patch: impl Fn(&mut UiTrackListRow),
) {
    let Some(vm) = vec_model(rows, "patch_track_row_by_id") else {
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
