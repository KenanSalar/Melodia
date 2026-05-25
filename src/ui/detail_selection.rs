//! Generic detail-view row selection: modifier-aware clicks plus surgical,
//! O(changed) row re-stamping.
//!
//! The Album / Artist / Genre / Playlist detail views all embed the same
//! reusable `TrackList` component and run byte-identical selection logic —
//! the only differences are the per-view Slint global type and the cached
//! Rust-side track list / applied-selection shadow. This module captures
//! that logic once, parameterised over the [`DetailSelectionView`] trait
//! (the handful of Slint accessors it touches) and a [`SelectionRefs`]
//! borrow of the two caches. Each view's `selection.rs` is a thin adapter
//! that supplies those.

use std::collections::{HashMap, HashSet};

use parking_lot::Mutex;
use slint::{Model, ModelRc, VecModel};

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AlbumDetail, ArtistDetail, GenreDetail, PlaylistDetail, TrackListRow as UiTrackListRow,
};

/// The per-detail-view Slint surface the generic selection logic drives.
/// Each detail global (`AlbumDetail`, `ArtistDetail`, …) implements this by
/// routing through its auto-generated `get_*` / `set_*` accessors.
pub trait DetailSelectionView {
    /// The shift-range anchor row index (`-1` = no anchor).
    fn anchor(&self) -> i32;
    fn set_anchor(&self, anchor: i32);
    /// The currently-selected track ids model.
    fn selected_ids(&self) -> ModelRc<i32>;
    fn replace_selected_ids(&self, ids: ModelRc<i32>);
    /// The displayed track-row model.
    fn track_rows(&self) -> ModelRc<UiTrackListRow>;
    /// Replace the displayed track-row model wholesale (downcast-miss
    /// fallback only — the model is normally mutated in place).
    fn replace_track_rows(&self, rows: ModelRc<UiTrackListRow>);
}

/// Generate a [`DetailSelectionView`] impl for a detail global. The trait
/// method names are deliberately distinct from Slint's `get_*` / `set_*`
/// accessors so the bodies are unambiguous.
macro_rules! impl_detail_selection_view {
    ($global:ident) => {
        impl DetailSelectionView for $global<'_> {
            fn anchor(&self) -> i32 {
                self.get_selection_anchor()
            }
            fn set_anchor(&self, anchor: i32) {
                self.set_selection_anchor(anchor);
            }
            fn selected_ids(&self) -> ModelRc<i32> {
                self.get_selected_ids()
            }
            fn replace_selected_ids(&self, ids: ModelRc<i32>) {
                self.set_selected_ids(ids);
            }
            fn track_rows(&self) -> ModelRc<UiTrackListRow> {
                self.get_tracks()
            }
            fn replace_track_rows(&self, rows: ModelRc<UiTrackListRow>) {
                self.set_tracks(rows);
            }
        }
    };
}

impl_detail_selection_view!(AlbumDetail);
impl_detail_selection_view!(ArtistDetail);
impl_detail_selection_view!(GenreDetail);
impl_detail_selection_view!(PlaylistDetail);

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
pub fn handle_select_row<V: DetailSelectionView>(
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
            let range: Vec<i32> = tracks[lo..=hi]
                .iter()
                .map(|t| clamp_i64_to_i32(t.id))
                .collect();
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
pub fn clear_selection<V: DetailSelectionView>(view: &V, refs: &SelectionRefs<'_>) {
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
pub fn apply_selection_to_rows<V: DetailSelectionView>(view: &V, refs: &SelectionRefs<'_>) {
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
    let index_of: HashMap<i32, usize> = refs
        .tracks
        .lock()
        .iter()
        .enumerate()
        .map(|(i, t)| (clamp_i64_to_i32(t.id), i))
        .collect();

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

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place. Falls
/// back to a fresh `ModelRc` only if the install step somehow didn't run.
pub fn write_selection<V: DetailSelectionView>(view: &V, ids: Vec<i32>) {
    let model = view.selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(ids);
    } else {
        view.replace_selected_ids(ModelRc::new(VecModel::from(ids)));
    }
}
