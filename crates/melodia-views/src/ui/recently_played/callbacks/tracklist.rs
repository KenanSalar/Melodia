//! The Songs tab's callbacks, and the filter both tabs share. There is no sort callback, recency
//! being the list's only order.

use std::sync::Arc;

use slint::{ComponentHandle, Global as _};

use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{
    play_displayed, wire_queue_actions, wire_row_selection, wire_toggle_column,
};
use crate::ui::callbacks::{collect_track_ids, spawn_play_then_shuffle};
use crate::ui::recently_played::{
    self as recently_played_ui_mod, RecentlyPlayedTab, RecentlyPlayedUi,
};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, RecentlyPlayed};

pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    wire_play_row(&g, state, rp_ui);
    wire_queue_actions(&g, state, collect_track_ids);
    wire_row_flags(ui, state, rp_ui);
    wire_filter(ui, state, rp_ui);
    wire_toggle_column(&g.as_weak(), state);
    wire_row_selection(&g.as_weak());
    wire_shuffle(&g, state, rp_ui);
}

/// Queues the filtered list, starting on the clicked track.
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
    // Both flags patch the cache and the row in place, so the scroll holds; neither changes
    // recency membership.
    {
        let ru = rp_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "recently_played::set_favorite",
        library::favorites::set_favorite, collect_track_ids,
        captures: [weak, ru],
        after: |id_vec, fav| {
            for id in &id_vec {
                ru.flip_track_favorite(*id, fav);
                recently_played_ui_mod::apply_row_favorite(&weak, *id, fav);
            }
        });
    }

    {
        let ru = rp_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "recently_played::set_rating",
        library::ratings::set_rating, collect_track_ids,
        captures: [weak, ru],
        after: |id_vec, rating| {
            for id in &id_vec {
                ru.flip_track_rating(*id, rating);
                recently_played_ui_mod::apply_row_rating(&weak, *id, rating);
            }
        });
    }
}

/// One needle over two lists: Songs re-walks its cache, and the grid requeries only while
/// mounted, the pick onto it clearing the needle anyway.
fn wire_filter(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let s = state.clone();
    let ru = rp_ui.clone();
    let weak = ui.as_weak();
    ui.global::<RecentlyPlayed>().on_filter_changed(move |text| {
        recently_played_ui_mod::set_filter(&ru, &text);
        recently_played_ui_mod::apply_filtered_tracks(&ru, &weak);
        if ru.section_active() && ru.active_tab() == RecentlyPlayedTab::MostPlayed {
            let Some(ui) = weak.upgrade() else { return };
            recently_played_ui_mod::requery_grid(&ui, &s, &ru);
        }
    });
}

fn wire_shuffle(g: &RecentlyPlayed<'_>, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let s = state.clone();
    let ru = rp_ui.clone();
    g.on_shuffle_all(move || {
        spawn_play_then_shuffle(&s, "recently_played::shuffle_all", ru.filtered_track_ids());
    });
}
