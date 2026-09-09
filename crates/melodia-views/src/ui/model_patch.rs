//! Surgical in-place patches on a Slint row `VecModel`.
//!
//! Every list surface (Tracks, Browse, the four detail views, Favorites,
//! Recently Played) patches a favorite/rating change into its visible rows the
//! same way, and the queue sheet patches two of its own: walk the model, mutate
//! the rows that matter, write back only those. This module owns that walk; the
//! per-view `apply_row_*` wrappers keep only their `upgrade_in_event_loop`
//! hop and the global they read.

use slint::{Model, ModelRc, VecModel};

use melodia_ui::TrackListRow as UiTrackListRow;

/// Hand every row to `patch` as a clone, writing back the ones it reports as changed.
///
/// **The `bool` is what keeps this affordable.** `set_row_data` fires `row_changed`, which drops
/// that row's `ListView` delegate, so a walk that writes unconditionally costs what a reset costs.
///
/// A model that is not a `VecModel` is logged under `label` rather than passed over: the rows and
/// whatever the caller has in hand disagree from there on, and nothing else says so. UI-thread only.
pub fn patch_rows_where<T: Clone + 'static>(
    rows: &ModelRc<T>,
    label: &str,
    mut patch: impl FnMut(&mut T) -> bool,
) {
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<T>>() else {
        log::warn!("{label}: VecModel downcast failed, rows left as they were");
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
    // The id is unique, so `done` is what stops a second row being handed to `patch` — the walk
    // itself runs on, a visible window being a screenful rather than the library.
    let mut done = false;
    patch_rows_where(rows, "patch_track_row_by_id", |row| {
        if done || i64::from(row.id) != id {
            return false;
        }
        patch(row);
        done = true;
        true
    });
}
