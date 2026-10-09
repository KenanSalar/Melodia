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

use slint::{ComponentHandle, Global as _};

use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{play_displayed, wire_queue_actions, wire_toggle_column};
use crate::ui::callbacks::{collect_track_ids, spawn_play_then_shuffle};
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, RecentlyPlayed};

/// Wire the list row / filter / column / selection / header callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    wire_play_row(&g, state, rp_ui);
    wire_queue_actions(&g, state, collect_track_ids);
    wire_row_flags(ui, state, rp_ui);
    wire_filter(ui, state, rp_ui);
    wire_toggle_column(&g.as_weak(), state);
    wire_selection(ui, rp_ui);
    wire_shuffle(&g, state, rp_ui);
}

/// Loads the filtered list into the queue and starts on the clicked track;
/// the header's Shuffle is the same call at index 0, plus a shuffle flip.
fn wire_play_row(g: &RecentlyPlayed<'_>, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let s = state.clone();
    let ru = rp_ui.clone();
    g.on_play_row(move |track_id, idx| {
        play_displayed(&s, view_id::RECENTLY_PLAYED, ru.filtered_track_ids(), track_id, idx);
    });
}

fn wire_row_flags(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();
    // toggle-row-favorite: flip in place (recency membership is independent of
    // the favorite flag, so the row stays). `set_favorite` bumps
    // `library_changed`; the lifecycle subscriber re-fetches. Multi-select
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
}

/// One needle, two caches — and they are walked on different threads because
/// they are bounded by different things. Songs is the 200-row recency set, so
/// its walk stays here. Most Played is whatever `get_most_played` returned,
/// uncapped and library-wide, so its walk goes to a worker and comes back
/// through `generation` to prove it still answers the needle on screen.
fn wire_filter(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let s = state.clone();
    let ru = rp_ui.clone();
    let weak = ui.as_weak();
    ui.global::<RecentlyPlayed>().on_filter_changed(move |text| {
        let generation = recently_played_ui_mod::set_filter(&ru, &text);
        recently_played_ui_mod::apply_filtered_tracks(&ru, &weak);

        let ru = ru.clone();
        let weak = weak.clone();
        s.runtime.spawn(async move {
            recently_played_ui_mod::apply_filtered_grid_settled(&ru, &weak, generation);
        });
    });
}

fn wire_selection(ui: &AppWindow, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    {
        let weak = ui.as_weak();
        let ru = rp_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::handle_select_row(&ui, &ru, idx, id, shift, ctrl);
        });
    }
    {
        let weak = ui.as_weak();
        let ru = rp_ui.clone();
        g.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::select_all(&ui, &ru);
        });
    }
    {
        let weak = ui.as_weak();
        let ru = rp_ui.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::clear_selection(&ui, &ru);
        });
    }
}

fn wire_shuffle(g: &RecentlyPlayed<'_>, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let s = state.clone();
    let ru = rp_ui.clone();
    g.on_shuffle_all(move || {
        spawn_play_then_shuffle(&s, "recently_played::shuffle_all", ru.filtered_track_ids());
    });
}
