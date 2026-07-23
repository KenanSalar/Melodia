//! Shared flat-list row-selection core: modifier-aware click math, the
//! diff-aware per-row `selected` stamper, and the persistent `selected-ids`
//! model writer.
//!
//! Tracks, Favorites, and Recently Played all implement the same click
//! semantics (plain click = single, Ctrl = toggle, Shift = range over the
//! displayed order); their per-view `selection.rs` files stay as thin
//! adapters that read/write their own Slint global and call in here. The
//! detail views keep `detail_selection.rs` — their selection is cache-indexed
//! with different complexity trade-offs.

use std::collections::HashSet;
use std::hash::BuildHasher;

use slint::{Model, ModelRc, VecModel};

use crate::TrackListRow as UiTrackListRow;

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
pub fn stamp_rows_selected<S: BuildHasher>(rows: &ModelRc<UiTrackListRow>, desired: &HashSet<i32, S>) {
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

#[cfg(test)]
#[path = "tests/list_selection_tests.rs"]
mod tests;
