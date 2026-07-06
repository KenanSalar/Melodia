//! `Favorites.*` All Songs list callbacks: row actions (play, queue,
//! favorite toggle), the filter pass, sort, column visibility, and
//! modifier-aware row selection. See [`super::wire_favorites`].

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::callbacks::collect_track_ids;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::ui::track_list_view::TrackListColumnState;
use crate::{AppWindow, Favorites};

/// Wire the All Songs row / filter / sort / selection callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // --- All Songs row actions ------------------------------------
    {
        let s = state.clone();
        g.on_play_row(move |track_id, _idx| {
            let s = s.clone();
            let id = i64::from(track_id);
            spawn_logged!(s, "favorites::play_row",
                library::queue::queue_append_unique(&s, id));
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
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_toggle_row_favorite(move |ids, fav| {
            let id_vec = collect_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let fu = fu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::favorites::set_favorite(&s, id_vec.clone(), fav).await {
                    log::warn!("favorites::set_favorite: {e}");
                    return;
                }
                for id in &id_vec {
                    fu.flip_or_remove_track(*id, fav);
                }
                favorites_ui_mod::apply_filtered_tracks(&fu, &weak);
            });
        });
    }

    // set-row-rating: rating is independent of favorite membership, so the
    // row stays — patch the cached rows and re-walk the filtered view.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_set_row_rating(move |ids, rating| {
            let id_vec = collect_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let fu = fu.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::ratings::set_rating(&s, id_vec.clone(), rating).await {
                    log::warn!("favorites::set_rating: {e}");
                    return;
                }
                for id in &id_vec {
                    fu.flip_track_rating(*id, rating);
                }
                favorites_ui_mod::apply_filtered_tracks(&fu, &weak);
            });
        });
    }

    // --- Filter / sort --------------------------------------------
    // Filter pass walks all three surfaces: All Songs tracklist
    // (title+artist+album), Most Played (title+artist) and Favorite
    // Artists (name). Each surface re-renders from its cached Rust
    // Vec, so the keystroke cost is `O(rows)` in-memory work — no
    // DB round-trip.
    {
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_filter_changed(move |text| {
            favorites_ui_mod::set_filter(&fu, text.to_string());
            favorites_ui_mod::apply_filtered_tracks(&fu, &weak);
            favorites_ui_mod::apply_filtered_strips(&fu, &weak);
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
