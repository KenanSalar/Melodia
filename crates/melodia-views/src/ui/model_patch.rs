//! Surgical single-row patches on Slint track-row `VecModel`s.
//!
//! Every list surface (Tracks, Browse, the four detail views, Favorites,
//! Recently Played) patches a favorite/rating change into its visible rows the
//! same way: walk the `VecModel<TrackListRow>`, find the row by id, mutate one
//! field, write it back with `set_row_data`. This module owns that walk; the
//! per-view `apply_row_*` wrappers keep only their `upgrade_in_event_loop`
//! hop and the global they read.

use slint::{Model, ModelRc, VecModel};

use melodia_ui::TrackListRow as UiTrackListRow;

/// Find the row with `id` and apply `patch` to a clone of it, writing back via
/// `set_row_data`. No-ops when the model isn't a `VecModel` or the id isn't
/// present. Only the affected row is written, so scroll position and
/// neighbouring rows stay put. UI-thread only.
pub fn patch_track_row_by_id(
    rows: &ModelRc<UiTrackListRow>,
    id: i64,
    patch: impl Fn(&mut UiTrackListRow),
) {
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        return;
    };
    for i in 0..vm.row_count() {
        let Some(mut r) = vm.row_data(i) else {
            continue;
        };
        if i64::from(r.id) == id {
            patch(&mut r);
            vm.set_row_data(i, r);
            break;
        }
    }
}
