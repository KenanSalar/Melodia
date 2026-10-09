//! `Favorites.*` Songs-tab callbacks: row actions (play, queue, favorite
//! toggle), the filter pass, sort, column visibility, and modifier-aware
//! row selection. See [`super::wire`].

use std::sync::Arc;

use slint::{ComponentHandle, Global as _, SharedString};

use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{play_displayed, wire_queue_actions, wire_toggle_column};
use crate::ui::callbacks::{collect_track_ids, next_sort, persist_view_sort};
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Favorites};

/// Wire the Songs tab's row / filter / sort / selection callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    wire_play_row(&g, state, fav_ui);
    wire_queue_actions(&g, state, collect_track_ids);
    wire_row_flags(ui, state, fav_ui);
    wire_filter(ui, fav_ui);
    wire_sort(ui, state, fav_ui);
    wire_toggle_column(&g.as_weak(), state);
    wire_selection(ui, fav_ui);
}

/// Loads the filtered list into the queue and starts on the clicked track;
/// the hero's Shuffle is the same call at index 0, plus a shuffle flip.
fn wire_play_row(g: &Favorites<'_>, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let s = state.clone();
    let fu = fav_ui.clone();
    g.on_play_row(move |track_id, idx| {
        play_displayed(&s, view_id::FAVORITES, fu.filtered_track_ids(), track_id, idx);
    });
}

fn wire_row_flags(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();
    // toggle-row-favorite: optimistic local removal (favourite ⇒ not-
    // favourite drops the row from the filtered view). `set_favorite`
    // bumps `library_changed`, which the lifecycle subscriber picks
    // up and re-fetches the list — covers the un-toggle-then-toggle race
    // where the user undoes the change quickly. Multi-select arrives
    // as `[int]`; single-row mode sends a 1-element array.
    {
        let fu = fav_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "favorites::set_favorite",
        library::favorites::set_favorite, collect_track_ids,
        captures: [weak, fu],
        after: |id_vec, fav| {
            for id in &id_vec {
                fu.flip_or_remove_track(*id, fav);
            }
            favorites_ui_mod::apply_filtered_tracks(&fu, &weak);
        });
    }

    // set-row-rating: rating is independent of favorite membership, so the
    // row stays put — patch the cached rows and the one visible row in place
    // (no full filtered-list rebuild).
    {
        let fu = fav_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "favorites::set_rating",
        library::ratings::set_rating, collect_track_ids,
        captures: [weak, fu],
        after: |id_vec, rating| {
            // Rating never removes the row (unlike the favorite toggle) and
            // there's no in-table rating sort, so patch each row in place
            // instead of rebuilding the whole filtered list.
            for id in &id_vec {
                fu.flip_track_rating(*id, rating);
                favorites_ui_mod::apply_row_rating(&weak, *id, rating);
            }
        });
    }
}

/// The filter is shared across the tabs, so a keystroke re-walks the
/// Songs cache (title+artist+album) and whichever grid is mounted —
/// Most Played (title+artist) or Favorite Artists (name). Each
/// re-renders from its cached Rust Vec, so the keystroke cost is
/// `O(rows)` in-memory work — no DB round-trip.
fn wire_filter(ui: &AppWindow, fav_ui: &Arc<FavoritesUi>) {
    let fu = fav_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Favorites>().on_filter_changed(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        favorites_ui_mod::set_filter(&fu, &text);
        favorites_ui_mod::apply_filtered_tracks(&fu, &weak);
        favorites_ui_mod::apply_filtered_grids_now(&ui, &fu);
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

        // In memory, like the Tracks view's header clicks: the rows are
        // already resident and their covers already warm, so only the
        // display permutation moves. This used to re-issue an unbounded
        // `SELECT` plus a full cover prewarm per click.
        favorites_ui_mod::resort_and_apply(&fu, &weak);
    });
}

/// Modifier-aware selection with
/// per-row `selected` flag re-stamping so the row checkbox + accent
/// highlight reflect each click. Mirrors `tracks::handle_select_row`
/// exactly so range-select (Shift), toggle (Ctrl), and the
/// single-row default all behave the same as in the Tracks view.
fn wire_selection(ui: &AppWindow, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    {
        let weak = ui.as_weak();
        let fu = fav_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            favorites_ui_mod::handle_select_row(&ui, &fu, idx, id, shift, ctrl);
        });
    }
    {
        let weak = ui.as_weak();
        let fu = fav_ui.clone();
        g.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            favorites_ui_mod::select_all(&ui, &fu);
        });
    }
    {
        let weak = ui.as_weak();
        let fu = fav_ui.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            favorites_ui_mod::clear_selection(&ui, &fu);
        });
    }
}
