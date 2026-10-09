//! Detail-row selection: [`crate::ui::list_selection`]'s click semantics, with the stamping done
//! O(changed) against the detail's own cache.
//!
//! The flat lists re-stamp every row per click; a detail can afford not to, its cache holding the
//! rows in model order, so an id resolves to a row index without cloning a Slint row.

use std::collections::{HashMap, HashSet};

use slint::{Model, VecModel};

use super::DetailCache;
use crate::ui::list_selection::{RowSelectionView, compute_click_selection, write_selection_ids};
use crate::ui::util::clamp_i64_to_i32;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_ui::TrackListRow as UiTrackListRow;

/// Computes the new selection for a row click and applies it. Runs on the UI thread.
pub fn handle_select_row<V: RowSelectionView>(
    view: &V,
    cache: &DetailCache,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let (new_selected, new_anchor) = compute_click_selection(
        view.anchor(),
        view.selected_ids().iter().collect(),
        // The cache is the display order, so the range needs no walk of the Slint model.
        || cache.tracks.lock().iter().map(|t| clamp_i64_to_i32(t.id)).collect(),
        idx,
        id,
        shift,
        ctrl,
    );
    write_selection(view, new_selected);
    view.set_anchor(new_anchor);
    apply_selection_to_rows(view, cache);
}

/// Takes every displayed row into the selection. The anchor is left where the last click put
/// it; [`crate::ui::list_selection::select_all_curated`] argues why.
///
/// **Deliberately not `list_selection::select_all_ids`**, which the flat lists take: the stamper
/// below is the O(changed) pass over this view's own cache, so folding the two walks into one
/// would trade a cheaper stamp for a full one.
pub fn select_all<V: RowSelectionView>(view: &V, cache: &DetailCache) {
    write_selection(view, crate::ui::list_selection::displayed_ids(&view.track_rows()));
    apply_selection_to_rows(view, cache);
}

/// Resets the selection (Escape, and the row menu's "Clear selection").
pub fn clear_selection<V: RowSelectionView>(view: &V, cache: &DetailCache) {
    write_selection(view, Vec::new());
    view.set_anchor(-1);
    apply_selection_to_rows(view, cache);
}

/// Empties the selection without walking the rows, for a model just rebuilt with every
/// `selected` false.
pub fn reset<V: RowSelectionView>(view: &V, cache: &DetailCache) {
    write_selection(view, Vec::new());
    view.set_anchor(-1);
    cache.applied_selection.lock().clear();
}

/// Re-stamps `selected` on only the rows whose membership changed since the last apply.
/// `VecModel::row_data` clones the whole row struct, so touching every row on every click would
/// clone up to N structs to flip one bool.
pub fn apply_selection_to_rows<V: RowSelectionView>(view: &V, cache: &DetailCache) {
    let desired: HashSet<i32> = view.selected_ids().iter().collect();
    let mut applied = cache.applied_selection.lock();

    let flipped: Vec<i32> = desired.symmetric_difference(&applied).copied().collect();
    if flipped.is_empty() {
        return;
    }

    let rows = view.track_rows();
    let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        return;
    };
    let index_of: HashMap<i32, usize> =
        cache.tracks.lock().iter().enumerate().map(|(i, t)| (clamp_i64_to_i32(t.id), i)).collect();

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

/// Drops from `selected-ids` every id `tracks` no longer carries. Each detail view's
/// re-fetch owes this before it hands the model swap on: the swap re-stamps the rows from
/// this set, so an id whose track is gone would otherwise keep the menu's count and the
/// applied shadow describing a track nothing can show.
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

fn write_selection<V: RowSelectionView>(view: &V, ids: Vec<i32>) {
    write_selection_ids(&view.selected_ids(), ids, |m| view.replace_selected_ids(m));
}
