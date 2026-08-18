//! Shared flat-list row-selection core: the [`DetailSelectionView`] surface the four detail
//! globals and the two curated pages are driven through, modifier-aware click math, the
//! diff-aware per-row `selected` stamper, and the persistent `selected-ids` model writer.
//!
//! Tracks, Favorites, and Recently Played all implement the same click
//! semantics (plain click = single, Ctrl = toggle, Shift = range over the
//! displayed order); their per-view `selection.rs` files stay as thin
//! adapters that read/write their own Slint global and call in here. Favorites and Recently
//! Played get the whole of it from the three `*_curated` functions below — their adapters were
//! the same four bodies character for character. **Tracks stays hand-written and should**: its
//! range branch walks the filtered cache (`current_ids_filtered`) where those two read the Slint
//! model directly.
//!
//! `detail_selection.rs` is the other consumer of the trait: same accessors, cache-indexed
//! selection with different complexity trade-offs.

use std::collections::HashSet;
use std::hash::BuildHasher;

use parking_lot::Mutex;
use slint::{Model, ModelRc, VecModel};

use crate::{
    AlbumDetail, ArtistDetail, Favorites, GenreDetail, PlaylistDetail, RecentlyPlayed,
    TrackListRow as UiTrackListRow,
};

/// Compute the new `(selected_ids, anchor)` for a row click.
///
/// * plain click → single-row selection, anchor moves to the clicked row
/// * Ctrl-click  → toggle this row in/out of the existing set
/// * Shift-click → range select from the current anchor to this row, in the
///   *displayed* order; the anchor stays put
///
/// `visible_ids` supplies the displayed-order id list and is only invoked in
/// the Shift branch, so callers with an expensive projection (Tracks walks its
/// filtered cache) don't pay it on plain clicks.
pub fn compute_click_selection(
    cur_anchor: i32,
    cur_selected: Vec<i32>,
    visible_ids: impl FnOnce() -> Vec<i32>,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) -> (Vec<i32>, i32) {
    if shift && cur_anchor >= 0 {
        let ids = visible_ids();
        if ids.is_empty() {
            (vec![id], idx)
        } else {
            let last = i32::try_from(ids.len().saturating_sub(1)).unwrap_or(i32::MAX);
            let lo = usize::try_from(cur_anchor.min(idx).clamp(0, last)).unwrap_or(0);
            let hi = usize::try_from(cur_anchor.max(idx).clamp(0, last)).unwrap_or(0);
            (ids[lo..=hi].to_vec(), cur_anchor)
        }
    } else if ctrl {
        let mut next = cur_selected;
        if let Some(pos) = next.iter().position(|&v| v == id) {
            next.remove(pos);
        } else {
            next.push(id);
        }
        (next, idx)
    } else {
        (vec![id], idx)
    }
}

/// Re-stamp `selected: bool` on every row in the visible
/// `VecModel<TrackListRow>` to match `desired`. Rows whose flag already
/// matches are skipped — `set_row_data` invalidates the `ListView` delegate
/// cache for that row, so a full rewrite would re-build every row component
/// on every click. UI-thread only.
pub fn stamp_rows_selected<S: BuildHasher>(
    rows: &ModelRc<UiTrackListRow>,
    desired: &HashSet<i32, S>,
) {
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        return;
    };
    for i in 0..vm.row_count() {
        let Some(mut r) = vm.row_data(i) else {
            continue;
        };
        let now = desired.contains(&r.id);
        if r.selected != now {
            r.selected = now;
            vm.set_row_data(i, r);
        }
    }
}

/// Mutate a persistent `selected-ids` `VecModel<i32>` in place. Falls back to
/// `install`ing a fresh `ModelRc` only if the install step somehow didn't run
/// (test harness, future hot-reload).
pub fn write_selection_ids(
    model: &ModelRc<i32>,
    ids: Vec<i32>,
    install: impl FnOnce(ModelRc<i32>),
) {
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(ids);
    } else {
        install(ModelRc::new(VecModel::from(ids)));
    }
}

/// Re-stamp selection onto a freshly-built row list before it's pushed into
/// the Slint model, so a filter change / library refresh doesn't drop the
/// user's existing selection.
pub fn restamp_selected<S: BuildHasher>(rows: &mut [UiTrackListRow], selected: &HashSet<i32, S>) {
    if selected.is_empty() {
        return;
    }
    for row in rows {
        if selected.contains(&row.id) {
            row.selected = true;
        }
    }
}

/// The per-view Slint surface both selection layers drive. Each global implements it by routing
/// through its auto-generated `get_*` / `set_*` accessors; the trait method names are deliberately
/// distinct from those so the bodies are unambiguous.
///
/// Here rather than in [`crate::ui::detail_selection`] because both layers name it and this is the
/// one they share — the *logic* differs (a detail view indexes its Rust cache and re-stamps
/// O(changed); a curated page reads the model and re-stamps O(rows)), the accessors don't. The
/// name is the older half of that: two of the six impls are curated pages, not detail views.
pub trait DetailSelectionView {
    /// The shift-range anchor row index (`-1` = no anchor).
    fn anchor(&self) -> i32;
    fn set_anchor(&self, anchor: i32);
    /// The currently-selected track ids model.
    fn selected_ids(&self) -> ModelRc<i32>;
    fn replace_selected_ids(&self, ids: ModelRc<i32>);
    /// The displayed track-row model.
    fn track_rows(&self) -> ModelRc<UiTrackListRow>;
    /// Replace the displayed track-row model wholesale (downcast-miss fallback only — the model
    /// is normally mutated in place).
    fn replace_track_rows(&self, rows: ModelRc<UiTrackListRow>);
}

/// Generate a [`DetailSelectionView`] impl for a Slint global.
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
impl_detail_selection_view!(Favorites);
impl_detail_selection_view!(GenreDetail);
impl_detail_selection_view!(PlaylistDetail);
impl_detail_selection_view!(RecentlyPlayed);

/// Compute the new selection set for a curated page's row click and apply it. Click semantics
/// match `tracks::handle_select_row` exactly. Runs on the UI thread (called from
/// `on_select_row`). The selection is mirrored into `applied` so the next `apply_filtered_tracks`
/// round can re-stamp the `selected` flag on freshly-built rows.
pub fn handle_curated_click<V: DetailSelectionView, S: BuildHasher>(
    view: &V,
    applied: &Mutex<HashSet<i32, S>>,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let (new_selected, new_anchor) = compute_click_selection(
        view.anchor(),
        view.selected_ids().iter().collect(),
        // Range select over the currently-displayed rows (post-filter, already in the same order
        // as the Slint `tracks` model). Read ids straight from the model rather than re-walking
        // the cached `tracks_all` + filter, so the range matches what the user is looking at even
        // mid-debounce — where a detail view, whose cache *is* the display order, indexes that.
        || {
            let rows = view.track_rows();
            (0..rows.row_count()).filter_map(|i| rows.row_data(i).map(|r| r.id)).collect()
        },
        idx,
        id,
        shift,
        ctrl,
    );

    let id_set: HashSet<i32> = new_selected.iter().copied().collect();
    write_curated_selection(view, new_selected);
    view.set_anchor(new_anchor);
    {
        // `clone_from` needs both sides on one hasher, and a public parameter may not name a
        // concrete one (`implicit_hasher`) — so refill, which keeps the allocation just the same.
        let mut applied = applied.lock();
        applied.clear();
        applied.extend(id_set.iter().copied());
    }

    // Re-stamp per-row `selected` flags so the checkbox + background highlight reflect the new
    // set immediately.
    stamp_rows_selected(&view.track_rows(), &id_set);
}

/// Reset a curated page's selection (called from the action-pill "Clear" button and
/// section-leave). Same diff-then-write shape as [`handle_curated_click`].
pub fn clear_curated_selection<V: DetailSelectionView, S: BuildHasher>(
    view: &V,
    applied: &Mutex<HashSet<i32, S>>,
) {
    write_curated_selection(view, Vec::new());
    view.set_anchor(-1);
    applied.lock().clear();
    stamp_rows_selected(&view.track_rows(), &HashSet::new());
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list before it's pushed into the
/// Slint model, so a filter change / library refresh doesn't drop the user's existing selection.
pub fn restamp_curated_rows<V: DetailSelectionView>(view: &V, rows: &mut [UiTrackListRow]) {
    let selected: HashSet<i32> = view.selected_ids().iter().collect();
    restamp_selected(rows, &selected);
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
fn write_curated_selection<V: DetailSelectionView>(view: &V, ids: Vec<i32>) {
    write_selection_ids(&view.selected_ids(), ids, |m| view.replace_selected_ids(m));
}

#[cfg(test)]
#[path = "tests/list_selection_tests.rs"]
mod tests;
