//! `RecentlyPlayed.*` Songs-tab callbacks: row actions (play, queue, favorite
//! toggle), the filter pass, column visibility, modifier-aware selection, and
//! the tab's Shuffle pill.
//!
//! There is no sort callback: the list is mounted `sortable: false`, so recency
//! is its only order and the filter re-walks the cached rows without
//! re-ordering them.
//!
//! The filter is the one thing here that isn't the Songs tab's alone — it is
//! shared with the Most Played grid, so a keystroke re-walks both caches.

use std::sync::Arc;

use slint::ComponentHandle;

use super::VIEW_ID;
use crate::library;
use crate::state::AppState;
use crate::ui::callbacks::{collect_track_ids, play_row_start, spawn_play_then_shuffle};
use crate::ui::callbacks::macros::{spawn_blocking_logged, spawn_logged, wire_row_flag};
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::ui::track_list_view::TrackListColumnState;
use crate::{AppWindow, RecentlyPlayed};

/// Wire the list row / filter / column / selection / header callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();

    // --- Row actions ----------------------------------------------
    // play-row loads the filtered list into the queue and starts on the
    // clicked track; the header's Shuffle is the same call at index 0, plus
    // a shuffle flip.
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        g.on_play_row(move |track_id, idx| {
            let ids = ru.filtered_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(s, "recently_played::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }
    {
        let s = state.clone();
        g.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "recently_played::play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }
    {
        let s = state.clone();
        g.on_add_to_queue(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "recently_played::add_to_queue",
                library::queue::queue_add_tracks(&s, id_vec));
        });
    }
    // toggle-row-favorite: flip in place (recency membership is independent of
    // the favorite flag, so the row stays). `set_favorite` bumps
    // `library_changed_tx`; the lifecycle subscriber re-fetches. Multi-select
    // arrives as `[int]`; single-row mode sends a 1-element array.
    {
        let ru = rp_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "recently_played::set_favorite",
            library::favorites::set_favorite, collect_track_ids,
            captures: [weak, ru],
            after: |id_vec, fav| {
                // Surgically patch each affected row (recency membership is
                // independent of the favorite flag, so the row stays put): no
                // 200-row rebuild, scroll position holds, no flash.
                for id in &id_vec {
                    ru.flip_track_favorite(*id, fav);
                    recently_played_ui_mod::apply_row_favorite(&weak, *id, fav);
                }
            });
    }

    // set-row-rating: flip in place (recency membership is fixed to the 200,
    // independent of rating), patching the cached row and the one visible row
    // (no full filtered-list rebuild).
    {
        let ru = rp_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "recently_played::set_rating",
            library::ratings::set_rating, collect_track_ids,
            captures: [weak, ru],
            after: |id_vec, rating| {
                // Rating never changes membership or sort (no rating column /
                // in-table rating sort), so patch each row in place.
                for id in &id_vec {
                    ru.flip_track_rating(*id, rating);
                    recently_played_ui_mod::apply_row_rating(&weak, *id, rating);
                }
            });
    }

    // --- Filter ---------------------------------------------------
    {
        let ru = rp_ui.clone();
        let weak = weak.clone();
        g.on_filter_changed(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::set_filter(&ru, &text);
            recently_played_ui_mod::apply_filtered_tracks(&ru, &weak);
            recently_played_ui_mod::apply_filtered_grid_now(&ui, &ru);
        });
    }

    // --- Column visibility ----------------------------------------
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<RecentlyPlayed>().snapshot_visible();
            let s_disk = s.clone();
            spawn_blocking_logged!(s, "recently_played::toggle_column",
                library::settings::update_view_columns(
                    &s_disk, VIEW_ID.to_owned(), columns));
        });
    }

    // --- Selection ------------------------------------------------
    {
        let weak = weak.clone();
        let ru = rp_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::handle_select_row(&ui, &ru, idx, id, shift, ctrl);
        });
    }
    {
        let weak = weak.clone();
        let ru = rp_ui.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::clear_selection(&ui, &ru);
        });
    }

    // --- Header pill: Shuffle -------------------------------------
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        g.on_shuffle_all(move || {
            spawn_play_then_shuffle(&s, "recently_played::shuffle_all", ru.filtered_track_ids());
        });
    }
}
