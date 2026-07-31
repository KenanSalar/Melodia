//! `Favorites.*` Songs-tab callbacks: row actions (play, queue, favorite
//! toggle), the filter pass, sort, column visibility, and modifier-aware
//! row selection. See [`super::wire_favorites`].

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::callbacks::{collect_track_ids, play_row_start};
use crate::ui::callbacks::macros::{spawn_logged, wire_row_flag};
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::track_list_view::TrackListColumnState;
use crate::{AppWindow, Favorites};

/// Wire the Songs tab's row / filter / sort / selection callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // --- Songs-tab row actions ------------------------------------
    // play-row loads the filtered list into the queue and starts on the
    // clicked track; the hero's Shuffle is the same call at index 0, plus
    // a shuffle flip.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        g.on_play_row(move |track_id, idx| {
            let ids = fu.filtered_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(s, "favorites::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }
    {
        let s = state.clone();
        g.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "favorites::play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }
    {
        let s = state.clone();
        g.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(s, "favorites::add_to_queue",
                library::queue::queue_add_tracks(&s, id_vec));
        });
    }
    // toggle-row-favorite: optimistic local removal (favourite ⇒ not-
    // favourite drops the row from the filtered view). `set_favorite`
    // bumps `library_changed_tx`, which the lifecycle subscriber picks
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

    // --- Filter / sort --------------------------------------------
    // The filter is shared across the tabs, so a keystroke re-walks the
    // Songs cache (title+artist+album) and whichever grid is mounted —
    // Most Played (title+artist) or Favorite Artists (name). Each
    // re-renders from its cached Rust Vec, so the keystroke cost is
    // `O(rows)` in-memory work — no DB round-trip.
    {
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_filter_changed(move |text| {
            favorites_ui_mod::set_filter(&fu, text.to_string());
            favorites_ui_mod::apply_filtered_tracks(&fu, &weak);
            favorites_ui_mod::apply_filtered_grids(&fu, &weak);
        });
    }
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Favorites>();
            let cur_field = g.get_sort_field();
            let cur_dir = g.get_sort_dir();
            let (new_field, new_dir_s) = if cur_field.as_str() == field.as_str() {
                let nd = if cur_dir.as_str() == "asc" { "desc" } else { "asc" };
                (field.to_string(), nd.to_string())
            } else {
                (field.to_string(), "asc".to_string())
            };
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir_s.as_str()));
            let new_dir = if new_dir_s == "desc" {
                SortDir::Desc
            } else {
                SortDir::Asc
            };
            favorites_ui_mod::set_sort(&fu, new_field.clone(), new_dir.clone());

            let s_disk = s.clone();
            let sort = ViewSort {
                field: new_field,
                dir: new_dir,
            };
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_view_sort(&s_disk, "favorites".to_owned(), sort)
                {
                    log::warn!("favorites::set_view_sort: {e}");
                }
            });

            let s_fetch = s.clone();
            let fu = fu.clone();
            let weak = weak.clone();
            spawn_logged!(s_fetch, "favorites::request_sort",
                favorites_ui_mod::refresh_tracks(&s_fetch, &fu, &weak));
        });
    }
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<Favorites>().snapshot_visible();
            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::update_view_columns(
                    &s_disk,
                    "favorites".to_owned(),
                    columns,
                ) {
                    log::warn!("favorites::toggle_column: {e}");
                }
            });
        });
    }
    // select-row / clear-selection — modifier-aware selection with
    // per-row `selected` flag re-stamping so the row checkbox + accent
    // highlight reflect each click. Mirrors `tracks::handle_select_row`
    // exactly so range-select (Shift), toggle (Ctrl), and the
    // single-row default all behave the same as in the Tracks view.
    {
        let weak = weak.clone();
        let fu = fav_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            favorites_ui_mod::handle_select_row(&ui, &fu, idx, id, shift, ctrl);
        });
    }
    {
        let weak = weak.clone();
        let fu = fav_ui.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            favorites_ui_mod::clear_selection(&ui, &fu);
        });
    }
}
