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
/// Selection is re-stamped, the shift-range anchor dropped — the model changed shape — and
/// the applied-selection shadow re-synced. UI thread.
pub fn apply_filtered_detail<V: RowSelectionView>(view: &V, refs: &FilterRefs<'_>) {
    let needle = refs.filter.lock().clone();

    let displayed: Vec<RsTrackListRow> = {
        let all = refs.all_tracks.lock();
        all.iter().filter(|r| track_matches(r, &needle)).cloned().collect()
    };
    let mut rows: Vec<UiTrackListRow> =
        displayed.iter().map(crate::ui::tracks::to_slint_track_list_row).collect();
    restamp_selection(view, &mut rows);
    *refs.tracks.lock() = displayed;
    install_tracks(view, rows);
    // A stale shift-range anchor would now index the wrong row; the `applied` shadow just
    // mirrors `selected-ids` for the next incremental click diff.
    view.set_anchor(-1);
    *refs.applied.lock() = view.selected_ids().iter().collect();
}

/// Swap the view's `tracks` `VecModel` contents in place, falling back to a fresh model if
/// the downcast fails — never expected, but it keeps the model from desyncing from the
/// cache on the dead path.
fn install_tracks<V: RowSelectionView>(view: &V, rows: Vec<UiTrackListRow>) {
    let model = view.track_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        vm.set_vec(rows);
    } else {
        view.replace_track_rows(ModelRc::new(VecModel::from(rows)));
    }
}
