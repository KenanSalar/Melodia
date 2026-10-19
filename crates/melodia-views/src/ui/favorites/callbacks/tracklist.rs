//! `Favorites.*` Songs-tab callbacks: row actions (play, queue, favorite
//! toggle), the filter pass, sort, column visibility, and modifier-aware
//! row selection. See [`super::wire`].

use std::sync::Arc;

use slint::{ComponentHandle, Global as _, SharedString};

use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{
    play_displayed, wire_queue_actions, wire_row_selection, wire_toggle_column,
};
use crate::ui::callbacks::{collect_track_ids, next_sort, persist_view_sort};
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Favorites};

/// Wire the Songs tab's row / filter / sort / selection callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    wire_play_row(ui, state);
    wire_queue_actions(&g, state, collect_track_ids);
    wire_row_flags(ui, state);
    wire_filter(ui, state, fav_ui);
    wire_sort(ui, state, fav_ui);
    wire_toggle_column(&g.as_weak(), state);
    wire_row_selection(&g.as_weak());
}

/// Loads the filtered list into the queue and starts on the clicked track;
/// the hero's Shuffle is the same call at index 0, plus a shuffle flip.
fn wire_play_row(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_play_row(move |track_id, idx| {
        let Some(ui) = weak.upgrade() else { return };
        let ids = favorites_ui_mod::displayed_ids(&ui.global::<Favorites>());
        play_displayed(&s, view_id::FAVORITES, ids, track_id, idx);
    });
}

fn wire_row_flags(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();
    // An unfavourite drops the rows on this tick; `set_favorite` has already bumped
    // `library_changed`, whose refresh reloads the list behind it, which also covers an undo
    // landing before the removal does. Multi-select arrives as `[int]`; single-row mode sends a
    // 1-element array.
    wire_row_flag!(g, on_toggle_row_favorite, state, "favorites::set_favorite",
    library::favorites::set_favorite, collect_track_ids,
    captures: [weak],
    after: |id_vec, fav| {
        if !fav {
            favorites_ui_mod::remove_rows(&weak, id_vec);
            return;
        }
        for id in id_vec {
            favorites_ui_mod::patch_row(&weak, id, |row| row.is_favorite = true);
        }
    });

    // Rating never changes membership and there's no in-table rating sort, so
    // each row is patched in place.
    wire_row_flag!(g, on_set_row_rating, state, "favorites::set_rating",
    library::ratings::set_rating, collect_track_ids,
    captures: [weak],
    after: |id_vec, rating| {
        for id in id_vec {
            favorites_ui_mod::patch_row(&weak, id, move |row| row.rating = rating);
        }
    });
}

/// The filter is shared across the tabs, so a keystroke reloads the ids of
/// whichever is mounted, the Songs list or a grid.
fn wire_filter(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let s = state.clone();
    let fu = fav_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_filter_changed(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        favorites_ui_mod::set_filter(&fu, &text);
        favorites_ui_mod::requery_tracks(&s, &fu, &weak);
        if fu.section_active() {
            favorites_ui_mod::requery_grid(&ui, &s, &fu);
        }
    });
}

fn wire_sort(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let s = state.clone();
    let fu = fav_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_request_sort(move |field| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let (new_field, new_dir) =
            next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
        g.set_sort_field(SharedString::from(new_field.as_str()));
        g.set_sort_dir(SharedString::from(new_dir.as_str()));
        favorites_ui_mod::set_sort(&fu, new_field.clone(), new_dir);
        persist_view_sort(&s, view_id::FAVORITES, new_field, new_dir);
        favorites_ui_mod::requery_tracks(&s, &fu, &weak);
    });
}
