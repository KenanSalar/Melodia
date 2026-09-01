//! The detail views' shared filter pass.
//!
//! The four detail views each filter their track list in memory, and the filter walk, the
//! selection re-stamp and the displayed-cache bookkeeping are identical — only the
//! per-view Slint global and the `*Ui` holding the caches differ. Captured once here over
//! the [`RowSelectionView`] trait and a [`FilterRefs`] borrow.
//!
//! Album / Genre / Playlist run the whole pass on the UI thread through
//! [`apply_filtered_detail`]. Artist does its own worker-thread row prep, also rebuilding
//! an Albums strip, so it reuses only [`restamp_selection`] and the predicate from
//! [`crate::ui::row_match`].

use std::collections::HashSet;

use parking_lot::Mutex;
use slint::{Model, ModelRc, VecModel};

use crate::TrackListRow as UiTrackListRow;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::ui::detail_selection::RowSelectionView;
use crate::ui::row_match::{Needle, track_matches};

/// Re-apply selection from the view's `selected-ids` onto freshly-built rows before they
/// are swapped in — `to_slint_track_list_row` defaults `selected` to `false`, so a
/// filter rebuild would otherwise drop every checkbox and accent highlight.
pub fn restamp_selection<V: RowSelectionView>(view: &V, rows: &mut [UiTrackListRow]) {
    let selected: HashSet<i32> = view.selected_ids().iter().collect();
    if selected.is_empty() {
        return;
    }
    for row in rows {
        if selected.contains(&row.id) {
            row.selected = true;
        }
    }
}

/// Borrows of the four caches a detail view keeps for its track filter. `tracks` is kept
/// in lockstep with the Slint model, so the selection and sort logic stays valid.
pub struct FilterRefs<'a> {
    /// Canonical full track set, in display-sort order — what the filter walks.
    pub all_tracks: &'a Mutex<Vec<RsTrackListRow>>,
    /// Displayed subset, overwritten by every apply.
    pub tracks: &'a Mutex<Vec<RsTrackListRow>>,
    /// Selection set currently stamped onto the Slint row model.
    pub applied: &'a Mutex<HashSet<i32>>,
    /// Live filter needle, folded by construction — the view's `set_filter` is the sole
    /// writer and `row_match::fold_needle` the only way to build one.
    pub filter: &'a Mutex<Needle>,
}

/// Re-walk `all_tracks` through the current needle, push the survivors into the Slint
/// model, and store the filtered subset back into `tracks` so it stays in lockstep.
/// Selection is re-stamped, the shift-range anchor dropped if row positions moved, and the
/// applied-selection shadow re-synced. UI thread.
///
/// Returns whether the model was reset. Anything a caller holds that is keyed on a row index
/// rather than an id — Playlist Detail's in-flight drag — is stale exactly then, and the reset
/// destroyed the row instance that would otherwise have cleared it.
pub fn apply_filtered_detail<V: RowSelectionView>(view: &V, refs: &FilterRefs<'_>) -> bool {
    let needle = refs.filter.lock().clone();

    let displayed: Vec<RsTrackListRow> = {
        let all = refs.all_tracks.lock();
        all.iter().filter(|r| track_matches(r, &needle)).cloned().collect()
    };
    let mut rows: Vec<UiTrackListRow> =
        displayed.iter().map(crate::ui::tracks::to_slint_track_list_row).collect();
    restamp_selection(view, &mut rows);
    *refs.tracks.lock() = displayed;
    // The anchor is a row index, so it only goes stale when positions moved — a refresh that
    // lands the same ids in the same slots leaves the user's shift range intact. The `applied`
    // shadow just mirrors `selected-ids` for the next incremental click diff.
    let was_reset = install_tracks(view, rows);
    if was_reset {
        view.set_anchor(-1);
    }
    *refs.applied.lock() = view.selected_ids().iter().collect();
    was_reset
}

/// Swap the view's `tracks` `VecModel` contents in place through the keyed diff, returning
/// whether the model was reset. Falling back to a fresh model on a failed downcast — never
/// expected — keeps it from desyncing from the cache, and counts as a reset.
///
/// Diffing is only correct here because the caller re-stamps selection onto `rows` first: the
/// comparison is whole-row, so a caller stamping afterwards would have its write skipped.
fn install_tracks<V: RowSelectionView>(view: &V, rows: Vec<UiTrackListRow>) -> bool {
    let model = view.track_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        crate::ui::model_diff::apply_rows_keyed(vm, rows, |r| r.id)
    } else {
        view.replace_track_rows(ModelRc::new(VecModel::from(rows)));
        true
    }
}
