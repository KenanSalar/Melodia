//! Generic detail-view row selection: modifier-aware clicks plus surgical,
//! O(changed) row re-stamping.
//!
//! The Album / Artist / Genre / Playlist detail views all embed the same
//! reusable `TrackList` component and run byte-identical selection logic —
//! the only differences are the per-view Slint global type and the cached
//! Rust-side track list / applied-selection shadow. This module captures
//! that logic once, parameterised over the [`RowSelectionView`] trait
//! (the handful of Slint accessors it touches) and a [`SelectionRefs`]
//! borrow of the two caches. Each view's `selection.rs` is a thin adapter
//! that supplies those.
//!
//! The trait itself lives in [`crate::ui::list_selection`], the layer under this one. It is
//! re-exported for [`crate::ui::detail_filter`], the one importer outside these two files, whose
//! import reads against the layer it belongs to rather than through it.

use std::collections::{HashMap, HashSet};

use parking_lot::Mutex;
use slint::{Model, ModelRc, VecModel};

use crate::TrackListRow as UiTrackListRow;
use crate::entities::track::TrackListRow as RsTrackListRow;
pub use crate::ui::list_selection::RowSelectionView;
use crate::ui::util::clamp_i64_to_i32;

/// Borrows of the Rust-side caches a detail view keeps alongside its Slint
/// model: the track rows in display order, and the selection set currently
/// *stamped* onto that model.
pub struct SelectionRefs<'a> {
    /// Cached detail track rows, in display order. Used to resolve a click
    /// index → id range and an id → row-index without cloning Slint rows.
    pub tracks: &'a Mutex<Vec<RsTrackListRow>>,
    /// The selection set currently stamped onto the Slint row model.
    /// [`apply_selection_to_rows`] diffs the desired selection against this.
    pub applied: &'a Mutex<HashSet<i32>>,
}

/// Compute the new selection state for a detail-row click and apply it.
/// Plain click selects one row; `ctrl` toggles a row; `shift` (with a live
/// anchor) range-selects over the displayed rows. Runs on the UI thread.
pub fn handle_select_row<V: RowSelectionView>(
    view: &V,
    refs: &SelectionRefs<'_>,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let cur_anchor = view.anchor();
    let cur_selected: Vec<i32> = view.selected_ids().iter().collect();

    let (new_selected, new_anchor) = if shift && cur_anchor >= 0 {
        // Range select over the displayed rows (`refs.tracks` order).
        let tracks = refs.tracks.lock();
        if tracks.is_empty() {
            (vec![id], idx)
        } else {
            let last = i32::try_from(tracks.len() - 1).unwrap_or(i32::MAX);
            let lo = usize::try_from(cur_anchor.min(idx).clamp(0, last)).unwrap_or(0);
            let hi = usize::try_from(cur_anchor.max(idx).clamp(0, last)).unwrap_or(0);
            let range: Vec<i32> = tracks[lo..=hi].iter().map(|t| clamp_i64_to_i32(t.id)).collect();
            (range, cur_anchor)
        }
    } else if ctrl {
        let mut next = cur_selected.clone();
        if let Some(pos) = next.iter().position(|&v| v == id) {
            next.remove(pos);
        } else {
            next.push(id);
        }
        (next, idx)
    } else {
        (vec![id], idx)
    };

    write_selection(view, new_selected);
    view.set_anchor(new_anchor);
    apply_selection_to_rows(view, refs);
}

/// Reset selection (called from the action-pill "Clear" button).
pub fn clear_selection<V: RowSelectionView>(view: &V, refs: &SelectionRefs<'_>) {
    write_selection(view, Vec::new());
    view.set_anchor(-1);
    apply_selection_to_rows(view, refs);
}

/// Re-stamp the `selected` flag on only the rows whose membership in the
/// selection set actually *changed* since the last apply — O(changed), not
/// O(rows). `VecModel::row_data` clones the whole row struct, so touching
/// every row on every click would clone up to N structs to flip one bool.
///
/// The id → row-index lookup is built from the Rust-side `refs.tracks`
/// cache (kept in the same display order as the Slint model), so reading a
/// row's id never costs a struct clone — only the genuinely flipped rows
/// are pulled out of the model.
pub fn apply_selection_to_rows<V: RowSelectionView>(view: &V, refs: &SelectionRefs<'_>) {
    let desired: HashSet<i32> = view.selected_ids().iter().collect();
    let mut applied = refs.applied.lock();

    // Ids whose selected-state flipped since the last apply.
    let flipped: Vec<i32> = desired.symmetric_difference(&applied).copied().collect();
    if flipped.is_empty() {
        return;
    }

    let rows = view.track_rows();
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        return;
    };
    let index_of: HashMap<i32, usize> =
        refs.tracks.lock().iter().enumerate().map(|(i, t)| (clamp_i64_to_i32(t.id), i)).collect();

    for id in flipped {
        let Some(&i) = index_of.get(&id) else {
            continue;
        };
        let Some(mut r) = vm.row_data(i) else {
            continue;
        };
        let now = desired.contains(&id);
        if r.selected != now {
            r.selected = now;
            vm.set_row_data(i, r);
        }
    }
    *applied = desired;
}

/// Drop from `selected-ids` every id `tracks` no longer carries. Each detail view's
/// re-fetch owes this before it hands the model swap on: the swap re-stamps the rows from
/// this set, so an id whose track is gone would otherwise keep the "{n} selected" pill and
/// the applied shadow describing a track nothing can show.
///
/// Bails on an empty selection, the steady state on the watcher tick that runs this, rather
/// than building a set over every track in the entity to prove nothing needed dropping. And
/// having built it, only writes when it actually dropped something: the write resets the
/// `[int]` model, which every mounted row reads the length of, so a tick that prunes nothing
/// would dirty a binding per visible row to store what was already there.
pub fn prune_selection_to<V: RowSelectionView>(view: &V, tracks: &[RsTrackListRow]) {
    let selected = view.selected_ids();
    if selected.row_count() == 0 {
        return;
    }
    let valid: HashSet<i32> = tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect();
    let kept: Vec<i32> = selected.iter().filter(|id| valid.contains(id)).collect();
    if kept.len() != selected.row_count() {
        write_selection(view, kept);
    }
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place. Falls
/// back to a fresh `ModelRc` only if the install step somehow didn't run.
pub fn write_selection<V: RowSelectionView>(view: &V, ids: Vec<i32>) {
    let model = view.selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(ids);
    } else {
        view.replace_selected_ids(ModelRc::new(VecModel::from(ids)));
    }
}
