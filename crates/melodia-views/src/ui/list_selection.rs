//! Shared flat-list row-selection core: the [`RowSelectionView`] surface the four detail
//! globals, Tracks, the two curated pages and Search are driven through, modifier-aware click
//! math, the diff-aware per-row `selected` stamper, and the persistent `selected-ids` model writer.
//!
//! Every list implements the same click semantics (plain click = single, Ctrl = toggle, Shift =
//! range over the displayed order). The four list views and the four details get the whole of it
//! from [`handle_row_click`], [`select_all_rows`] and [`clear_row_selection`], wired once by
//! `callbacks::track_list::wire_row_selection`.

use std::collections::HashSet;
use std::hash::BuildHasher;

use slint::{Model, ModelRc, VecModel};

use crate::ui::paged_rows::track_rows::TrackRows;
use melodia_ui::{
    AlbumDetail, ArtistDetail, Favorites, GenreDetail, PlaylistDetail, RecentlyPlayed, Search,
    TrackListRow as UiTrackListRow, Tracks,
};

/// The anchor of a list nothing has been clicked in since its selection was last cleared.
pub const NO_ANCHOR: i32 = -1;

/// Compute the new `(selected_ids, anchor)` for a row click on a list picking by id.
///
/// [`click_selection`] with the id lists' two conventions: [`NO_ANCHOR`] for no anchor, and no
/// row listed as `0` ever picked, which is how Browse's disk-only rows keep their place in the
/// displayed order without joining a range. Browse refuses a click on one of those itself.
pub fn compute_click_selection(
    anchor: i32,
    cur_selected: Vec<i32>,
    visible_ids: impl FnOnce() -> Vec<i32>,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) -> (Vec<i32>, i32) {
    let anchor = (anchor != NO_ANCHOR).then_some(anchor);
    let (picked, anchor) = click_selection(anchor, cur_selected, visible_ids, idx, id, shift, ctrl);
    (picked.into_iter().filter(|&picked| picked != 0).collect(), anchor)
}

/// Compute the new `(picked, anchor)` for a row click, over whatever key the list picks rows by.
///
/// The anchor is the **key** of the row last clicked rather than its position, so a range runs
/// from that row wherever a re-sort, a refilter or a reorder has since moved it.
///
/// * plain click → single-row selection, the anchor moving to the clicked row
/// * Ctrl-click  → toggle this row in or out of the set, the anchor moving to it
/// * Shift-click → the displayed rows from the anchor's to this one, the anchor staying put; with
///   no anchor, or the anchor's row filtered out, it is a plain click
///
/// `visible_keys` supplies the displayed-order key list and is only invoked in the Shift branch,
/// so callers with an expensive projection don't pay it on plain clicks. `idx` is the clicked
/// row's position, trusted where it names `key` and looked up by key otherwise.
pub fn click_selection<K: Copy + PartialEq>(
    anchor: Option<K>,
    cur_selected: Vec<K>,
    visible_keys: impl FnOnce() -> Vec<K>,
    idx: i32,
    key: K,
    shift: bool,
    ctrl: bool,
) -> (Vec<K>, K) {
    if shift && let Some(anchor) = anchor {
        let keys = visible_keys();
        let from = keys.iter().position(|&shown| shown == anchor);
        let to = usize::try_from(idx)
            .ok()
            .filter(|&at| keys.get(at) == Some(&key))
            .or_else(|| keys.iter().position(|&shown| shown == key));
        if let (Some(from), Some(to)) = (from, to) {
            return (keys[from.min(to)..=from.max(to)].to_vec(), anchor);
        }
        return (vec![key], key);
    }
    if ctrl {
        let mut next = cur_selected;
        if let Some(pos) = next.iter().position(|&picked| picked == key) {
            next.remove(pos);
        } else {
            next.push(key);
        }
        return (next, key);
    }
    (vec![key], key)
}

/// Re-stamp `selected: bool` on every visible row to match `desired`. Rows
/// whose flag already matches are skipped — `set_row_data` invalidates the
/// `ListView` delegate cache for that row, so a full rewrite would re-build
/// every row component on every click. A [`TrackRows`] keeps the set
/// itself and makes the same skip without fetching a row. UI-thread only.
pub fn stamp_rows_selected<S: BuildHasher>(
    rows: &ModelRc<UiTrackListRow>,
    desired: &HashSet<i32, S>,
) {
    if let Some(model) = TrackRows::of(rows) {
        model.select(desired);
        return;
    }
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

/// Every id a row model is currently showing, in display order.
///
/// Read off the Slint model rather than any view's cache, so one projection answers for every
/// flat list: the model *is* the display order, post-filter and post-sort, whatever each view
/// rebuilt it from. Disk-only Browse rows are dropped here rather than at that view — they arrive
/// `enabled == false` and all share `id == 0`, which an id-keyed selection cannot hold.
pub fn displayed_ids(rows: &ModelRc<UiTrackListRow>) -> Vec<i32> {
    (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .filter(|r| r.enabled && r.id != 0)
        .map(|r| r.id)
        .collect()
}

/// Every displayed id, stamping `selected` on the rows in the same walk.
///
/// **Select All is the one selection whose predicate is the row's own**, so it needs no id set to
/// test against and no second pass: [`displayed_ids`] followed by [`stamp_rows_selected`] clones
/// every row twice to answer what one walk answers, and Slint's `Model` has no borrowing
/// accessor to make either clone free. Keeps that function's skip-if-unchanged guard for the
/// same reason it has one.
///
/// A [`TrackRows`] answers from its keyset, every id in it being a library row, and walking it
/// would fetch every page. Falls back to [`displayed_ids`] where the model is neither, which is
/// [`stamp_rows_selected`]'s own early return: there is nothing to stamp either way.
pub fn select_all_ids(rows: &ModelRc<UiTrackListRow>) -> Vec<i32> {
    if let Some(model) = TrackRows::of(rows) {
        return model.select_all();
    }
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        return displayed_ids(rows);
    };
    let mut ids = Vec::with_capacity(vm.row_count());
    for i in 0..vm.row_count() {
        let Some(mut r) = vm.row_data(i) else {
            continue;
        };
        let wanted = r.enabled && r.id != 0;
        if wanted {
            ids.push(r.id);
        }
        if r.selected != wanted {
            r.selected = wanted;
            vm.set_row_data(i, r);
        }
    }
    ids
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

/// The per-view Slint surface the selection helpers drive. Each global implements it by routing
/// through its auto-generated `get_*` / `set_*` accessors; the trait method names are deliberately
/// distinct from those so the bodies are unambiguous.
pub trait RowSelectionView {
    /// The id of the row a shift-range runs from, or [`NO_ANCHOR`].
    fn anchor(&self) -> i32;
    fn set_anchor(&self, anchor: i32);
    /// The currently-selected track ids model.
    fn selected_ids(&self) -> ModelRc<i32>;
    fn replace_selected_ids(&self, ids: ModelRc<i32>);
    /// The displayed track-row model.
    fn track_rows(&self) -> ModelRc<UiTrackListRow>;

    fn bind_select_row(&self, f: impl FnMut(i32, i32, bool, bool) + 'static);
    fn bind_select_all(&self, f: impl FnMut() + 'static);
    fn bind_clear_selection(&self, f: impl FnMut() + 'static);
}

/// Generate a [`RowSelectionView`] impl for a Slint global. Tracks names its row model `rows`
/// where every other list says `tracks`.
macro_rules! impl_row_selection_view {
    ($global:ident) => {
        impl_row_selection_view!($global, get_tracks);
    };
    ($global:ident, $get_rows:ident) => {
        impl RowSelectionView for $global<'_> {
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
                self.$get_rows()
            }
            fn bind_select_row(&self, f: impl FnMut(i32, i32, bool, bool) + 'static) {
                self.on_select_row(f);
            }
            fn bind_select_all(&self, f: impl FnMut() + 'static) {
                self.on_select_all(f);
            }
            fn bind_clear_selection(&self, f: impl FnMut() + 'static) {
                self.on_clear_selection(f);
            }
        }
    };
}

impl_row_selection_view!(AlbumDetail);
impl_row_selection_view!(ArtistDetail);
impl_row_selection_view!(Favorites);
impl_row_selection_view!(GenreDetail);
impl_row_selection_view!(PlaylistDetail);
impl_row_selection_view!(RecentlyPlayed);
impl_row_selection_view!(Search);
impl_row_selection_view!(Tracks, get_rows);

/// Compute the new selection set for a list view's row click and apply it, on the UI thread.
pub fn handle_row_click<V: RowSelectionView>(view: &V, idx: i32, id: i32, shift: bool, ctrl: bool) {
    let (new_selected, new_anchor) = compute_click_selection(
        view.anchor(),
        view.selected_ids().iter().collect(),
        // Range select over the displayed rows, read straight from the model rather than a cache
        // refiltered by the box's newer text, so the range matches what the user is looking at
        // even mid-debounce. A detail view, whose cache *is* the display order, indexes that.
        || row_ids(&view.track_rows()),
        idx,
        id,
        shift,
        ctrl,
    );

    let id_set: HashSet<i32> = new_selected.iter().copied().collect();
    write_row_selection(view, new_selected);
    view.set_anchor(new_anchor);

    // Re-stamp per-row `selected` flags so the checkbox + background highlight reflect the new
    // set immediately.
    stamp_rows_selected(&view.track_rows(), &id_set);
}

/// Take every displayed row into a list view's selection.
///
/// **The anchor is left where the last click put it**, unlike the clear below: Select All is not
/// itself a click, so a shift-click after one should still range from whatever the user picked.
/// The queue's own select-all has said the same since it was written.
pub fn select_all_rows<V: RowSelectionView>(view: &V) {
    let ids = select_all_ids(&view.track_rows());
    write_row_selection(view, ids);
}

/// Reset a list view's selection (Escape, the row menu's "Clear selection", and
/// section-leave). Same diff-then-write shape as [`handle_row_click`].
pub fn clear_row_selection<V: RowSelectionView>(view: &V) {
    write_row_selection(view, Vec::new());
    view.set_anchor(NO_ANCHOR);
    stamp_rows_selected(&view.track_rows(), &HashSet::new());
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list before it's pushed into the
/// Slint model, so a filter change / library refresh doesn't drop the user's existing selection.
pub fn restamp_row_selection<V: RowSelectionView>(view: &V, rows: &mut [UiTrackListRow]) {
    let selected: HashSet<i32> = view.selected_ids().iter().collect();
    restamp_selected(rows, &selected);
}

/// Drops every id the list no longer holds from a view's selection, the anchor with it, when the
/// set behind the list changes. UI-thread only.
///
/// `held` is the whole set, never the rows a filter left on screen: a filter hides a selection,
/// which the next refilter paints again, where a track gone from the set is one the next batch
/// action would otherwise still act on. Built only where a selection or an anchor is held.
pub fn keep_selection_within<V: RowSelectionView>(view: &V, held: impl FnOnce() -> HashSet<i32>) {
    let selected: Vec<i32> = view.selected_ids().iter().collect();
    let anchor = view.anchor();
    if selected.is_empty() && anchor == NO_ANCHOR {
        return;
    }
    let held = held();
    if !held.contains(&anchor) {
        view.set_anchor(NO_ANCHOR);
    }
    let kept: Vec<i32> = selected.iter().copied().filter(|id| held.contains(id)).collect();
    if kept.len() == selected.len() {
        return;
    }
    let kept_set: HashSet<i32> = kept.iter().copied().collect();
    write_row_selection(view, kept);
    stamp_rows_selected(&view.track_rows(), &kept_set);
}

/// Drops `removed` from a view's selection, the anchor with it, when rows leave a list ahead of
/// its refetch. Every other pick stays, the ones a filter hides included. UI-thread only.
pub fn forget_rows<V: RowSelectionView, S: BuildHasher>(view: &V, removed: &HashSet<i32, S>) {
    let anchor = view.anchor();
    keep_selection_within(view, || {
        view.selected_ids().iter().chain([anchor]).filter(|id| !removed.contains(id)).collect()
    });
}

/// Every row's id in display order, one per row so a click's index still names its row.
fn row_ids(rows: &ModelRc<UiTrackListRow>) -> Vec<i32> {
    match TrackRows::of(rows) {
        Some(model) => model.row_ids(),
        None => (0..rows.row_count()).filter_map(|i| rows.row_data(i).map(|r| r.id)).collect(),
    }
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
fn write_row_selection<V: RowSelectionView>(view: &V, ids: Vec<i32>) {
    write_selection_ids(&view.selected_ids(), ids, |m| view.replace_selected_ids(m));
}

#[cfg(test)]
#[path = "tests/list_selection_tests.rs"]
mod tests;
