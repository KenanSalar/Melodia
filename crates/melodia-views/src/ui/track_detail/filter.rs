//! The detail views' in-memory filter pass. Artist Detail runs it and then narrows its own
//! Albums strip, that carousel being the one thing the four do not share.

use slint::{Model, ModelRc, VecModel};

use super::DetailCache;
use crate::ui::list_selection::{RowSelectionView, restamp_curated_rows};
use crate::ui::row_match::track_matches;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_ui::TrackListRow as UiTrackListRow;

/// Re-walks the canonical set through the needle, pushes the survivors into the model and keeps
/// the displayed cache in lockstep. Selection is re-stamped and the shift-range anchor dropped
/// if row positions moved. UI thread.
///
/// Returns whether the model was reset. Anything a caller holds that is keyed on a row index
/// rather than an id, Playlist Detail's in-flight drag, is stale exactly then, and the reset
/// destroyed the row instance that would otherwise have cleared it.
pub fn apply_filtered_detail<V: RowSelectionView>(view: &V, cache: &DetailCache) -> bool {
    let needle = cache.needle();

    let displayed: Vec<RsTrackListRow> = {
        let all = cache.all_tracks.lock();
        all.iter().filter(|r| track_matches(r, &needle)).cloned().collect()
    };
    let mut rows: Vec<UiTrackListRow> = crate::ui::tracks::to_slint_track_list_rows(&displayed);
    restamp_curated_rows(view, &mut rows);
    *cache.tracks.lock() = displayed;
    // The anchor is a row index, so it only goes stale when positions moved: a refresh that
    // lands the same ids in the same slots leaves the user's shift range intact.
    let was_reset = install_tracks(view, rows);
    if was_reset {
        view.set_anchor(-1);
    }
    *cache.applied_selection.lock() = view.selected_ids().iter().collect();
    was_reset
}

/// Swaps the view's `tracks` model contents in place through the keyed diff, returning whether
/// the model was reset. Falling back to a fresh model on a failed downcast, never expected,
/// keeps it from desyncing from the cache, and counts as a reset.
///
/// `rows` must already carry the selection they should end up with: the comparison is
/// whole-row, so a caller stamping afterwards would have its write skipped.
pub fn install_tracks<V: RowSelectionView>(view: &V, rows: Vec<UiTrackListRow>) -> bool {
    let model = view.track_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        crate::ui::model_diff::apply_rows_keyed(vm, rows, |r| r.id)
    } else {
        view.replace_track_rows(ModelRc::new(VecModel::from(rows)));
        true
    }
}
