//! Favorites All-Songs row selection: modifier-aware clicks, clear, and
//! the per-row `selected` flag writer that drives row highlight +
//! checkbox state. Thin adapter over [`crate::ui::list_selection`] — the
//! `TrackList` component reads each row's `selected: bool` for its
//! checkbox tick and accent-tinted background, so a click that only
//! updates `selected-ids` (without re-stamping the per-row flag) leaves
//! the row visually un-selected even though it counts toward the "{n}
//! selected" chip.

use std::collections::HashSet;

use slint::{ComponentHandle, Model, ModelRc};

use super::FavoritesUi;
use crate::ui::list_selection;
use crate::{AppWindow, Favorites, TrackListRow as UiTrackListRow};

/// Compute the new selection set for a row click and apply it. Click
/// semantics match `tracks::handle_select_row` exactly. Runs on the UI
/// thread (called from `on_select_row`). The selection `HashSet` is
/// mirrored into `FavoritesUi::state().applied_selection` so the next
/// `apply_filtered_tracks` round can re-stamp the `selected` flag on
/// freshly-built rows.
pub fn handle_select_row(
    ui: &AppWindow,
    fav_ui: &FavoritesUi,
    idx: i32,
    id: i32,
    shift: bool,
    ctrl: bool,
) {
    let g = ui.global::<Favorites>();
    let cur_anchor = g.get_selection_anchor();
    let cur_selected: Vec<i32> = g.get_selected_ids().iter().collect();

    let (new_selected, new_anchor) = list_selection::compute_click_selection(
        cur_anchor,
        cur_selected,
        // Range select over the currently-displayed rows (post-filter,
        // already in the same order as the Slint `tracks` model). Read ids
        // straight from the model rather than re-walking the cached
        // `tracks_all` + filter, so the range matches what the user is
        // looking at even mid-debounce.
        || {
            let rows = g.get_tracks();
            (0..rows.row_count())
                .filter_map(|i| rows.row_data(i).map(|r| r.id))
                .collect()
        },
        idx,
        id,
        shift,
        ctrl,
    );

    let id_set: HashSet<i32> = new_selected.iter().copied().collect();
    write_selection(&g, new_selected);
    g.set_selection_anchor(new_anchor);
    (*fav_ui.state().applied_selection.lock()).clone_from(&id_set);

    // Re-stamp per-row `selected` flags so the checkbox + background
    // highlight reflect the new set immediately.
    list_selection::stamp_rows_selected(&g.get_tracks(), &id_set);
}

/// Reset selection (called from the action-pill "Clear" button and
/// section-leave). Same diff-then-write shape as `handle_select_row`.
pub fn clear_selection(ui: &AppWindow, fav_ui: &FavoritesUi) {
    let g = ui.global::<Favorites>();
    write_selection(&g, Vec::new());
    g.set_selection_anchor(-1);
    fav_ui.state().applied_selection.lock().clear();
    list_selection::stamp_rows_selected(&g.get_tracks(), &HashSet::new());
}

/// Mutate the persistent `selected-ids` `VecModel<i32>` in place.
pub(super) fn write_selection(g: &Favorites, ids: Vec<i32>) {
    list_selection::write_selection_ids(&g.get_selected_ids(), ids, |m: ModelRc<i32>| {
        g.set_selected_ids(m);
    });
}

/// UI-thread-only: re-stamp selection onto a freshly-built row list
/// before it's pushed into the Slint model. Invoked from
/// `tracks::apply_filtered_tracks` so a filter change / library
/// refresh doesn't drop the user's existing selection.
pub fn restamp_rows(g: &Favorites, rows: &mut [UiTrackListRow]) {
    let selected_set: HashSet<i32> = g.get_selected_ids().iter().collect();
    list_selection::restamp_selected(rows, &selected_set);
}
