//! The Browse file list: play, queue, favorite, rating, sort, selection.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::library;
use crate::services::view_state::ViewStateData;
use crate::state::AppState;
use crate::ui::browse::{self as browse_ui_mod, BrowseUi};
use crate::ui::callbacks::macros::{spawn_logged, wire_row_flag};
use crate::ui::callbacks::{
    collect_nonzero_track_ids, next_sort, persist_view_sort, persisted_sort, play_row_start,
};
use crate::ui::track_list_view::view_id;
use crate::{AppWindow, Browse};

/// Wire the list's own callbacks, and seed the sort the first navigation applies.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    browse_ui: &Arc<BrowseUi>,
) {
    let g = ui.global::<Browse>();
    let weak = ui.as_weak();

    // Seed the sort header + the `BrowseUi` sort cache from the persisted
    // `view_sort["browse"]` so the first folder navigation sorts with it.
    if let Some(sort) = persisted_sort(view_state, view_id::BROWSE) {
        g.set_sort_field(SharedString::from(sort.field.as_str()));
        g.set_sort_dir(SharedString::from(sort.dir.as_str()));
        browse_ui.set_sort(sort.field.clone(), sort.dir.as_str().to_owned());
    }

    // play-row: double-click loads every in-library file in this folder into
    // the queue and starts on the clicked one. Disk-only rows (`id == 0`)
    // aren't in the library and are ignored — they also *displace* the row
    // index, since `current_in_library_ids` drops them, which is the case
    // `play_row_start`'s lookup-by-id fallback exists for.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        g.on_play_row(move |track_id, idx| {
            let id = i64::from(track_id);
            if id == 0 {
                return;
            }
            let ids = bu.current_in_library_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, id, idx);
            let s = s.clone();
            spawn_logged!(
                s,
                "browse::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
            );
        });
    }

    // play-next / add-to-queue: context-menu actions. Single-row and
    // multi-select both flow through the same callback as `[int]`.
    // Disk-only rows (`id == 0`) are filtered out — they aren't in the
    // library and can't be queued. (Selection-level gate at
    // `browse/selection.rs` already keeps disk-only ids out of
    // `Browse.selected-ids`; this filter is a belt-and-braces for
    // single-row mode if anything ever changes upstream.)
    {
        let s = state.clone();
        g.on_play_next(move |ids| {
            let id_vec = collect_nonzero_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "browse::play_next", library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        g.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).filter(|&id| id != 0).collect();
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "browse::add_to_queue", library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite / set-row-rating: write through, then surgically
    // update each row (no re-fetch, so scroll position holds and there's no
    // flash). Disk-only rows have id 0 and are filtered out by
    // `collect_nonzero_track_ids`; rating never changes list membership.
    {
        let bu = browse_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "browse::set_favorite",
        library::favorites::set_favorite, collect_nonzero_track_ids,
        captures: [weak, bu],
        after: |id_vec, fav| {
            for id in &id_vec {
                bu.flip_favorite(*id, fav);
                browse_ui_mod::apply_row_favorite(&weak, *id, fav);
            }
        });
    }
    {
        let bu = browse_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "browse::set_rating",
        library::ratings::set_rating, collect_nonzero_track_ids,
        captures: [weak, bu],
        after: |id_vec, rating| {
            for id in &id_vec {
                bu.flip_rating(*id, rating);
                browse_ui_mod::apply_row_rating(&weak, *id, rating);
            }
        });
    }

    // request-sort: clicking a header column. Same field flips dir; new
    // field resets to ascending. Browse sorts in-memory (it mixes
    // disk-only + DB files) — no DB round-trip.
    {
        let s = state.clone();
        let bu = browse_ui.clone();
        let weak = weak.clone();
        g.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Browse>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            bu.set_sort(new_field.clone(), new_dir.as_str().to_owned());
            persist_view_sort(&s, view_id::BROWSE, new_field, new_dir);
            browse_ui_mod::resort_and_apply(&ui, &bu);
        });
    }

    // select-row / clear-selection: modifier-aware selection. The new
    // selected set is computed in Rust (Slint expressions can't iterate
    // a model for a membership check); disk-only rows are never
    // selectable. Mirrors the Tracks view.
    {
        let weak = weak.clone();
        let bu = browse_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::handle_select_row(&ui, &bu, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            browse_ui_mod::clear_selection(&ui);
        });
    }
}
