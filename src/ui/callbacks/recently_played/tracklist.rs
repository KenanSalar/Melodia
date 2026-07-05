//! `RecentlyPlayed.*` list callbacks: row actions (play, queue, favorite
//! toggle), the filter pass, in-memory sort, column visibility, modifier-aware
//! selection, and the header Play All / Shuffle pills.
//!
//! Sorting differs from Favorites: the recency set has fixed membership, so a
//! column sort re-walks the cached rows in memory (`apply_filtered_tracks`)
//! rather than re-querying.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use super::VIEW_ID;
use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::callbacks::collect_track_ids;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::ui::track_list_view::TrackListColumnState;
use crate::{AppWindow, RecentlyPlayed};

/// Wire the list row / filter / sort / selection / header callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();

    // --- Row actions ----------------------------------------------
    {
        let s = state.clone();
        g.on_play_row(move |track_id, _idx| {
            let s = s.clone();
            let id = i64::from(track_id);
            spawn_logged!(s, "recently_played::play_row",
                library::queue::queue_append_unique(&s, id));
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
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
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
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        g.on_toggle_row_favorite(move |ids, fav| {
            let id_vec = collect_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let ru = ru.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::favorites::set_favorite(&s, id_vec.clone(), fav).await {
                    log::warn!("recently_played::set_favorite: {e}");
                    return;
                }
                for id in &id_vec {
                    ru.flip_track_favorite(*id, fav);
                }
                recently_played_ui_mod::apply_filtered_tracks(&ru, &weak);
            });
        });
    }

    // --- Filter ---------------------------------------------------
    {
        let ru = rp_ui.clone();
        let weak = weak.clone();
        g.on_filter_changed(move |text| {
            recently_played_ui_mod::set_filter(&ru, text.to_string());
            recently_played_ui_mod::apply_filtered_tracks(&ru, &weak);
            recently_played_ui_mod::apply_filtered_strips(&ru, &weak);
        });
    }

    // --- Sort (in-memory over the fixed recency set) --------------
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        g.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<RecentlyPlayed>();
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
            recently_played_ui_mod::set_sort(&ru, new_field.clone(), new_dir.clone());

            // Persist so the next launch lands on the same order.
            let s_disk = s.clone();
            let sort = ViewSort {
                field: new_field,
                dir: new_dir,
            };
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_view_sort(&s_disk, VIEW_ID.to_owned(), sort)
                {
                    log::warn!("recently_played::set_view_sort: {e}");
                }
            });

            // In-memory re-sort — no re-fetch (membership is fixed).
            recently_played_ui_mod::apply_filtered_tracks(&ru, &weak);
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
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::update_view_columns(
                    &s_disk,
                    VIEW_ID.to_owned(),
                    columns,
                ) {
                    log::warn!("recently_played::toggle_column: {e}");
                }
            });
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

    // --- Header pills: Play All / Shuffle -------------------------
    // Enqueue the filtered set in display order starting at index 0.
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        g.on_play_all(move || {
            let ids = ru.filtered_track_ids();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "recently_played::play_all",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)));
        });
    }
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        g.on_shuffle_all(move || {
            let ids = ru.filtered_track_ids();
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) =
                    library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)).await
                {
                    log::warn!("recently_played::shuffle_all play: {e}");
                    return;
                }
                let shuffle_on = {
                    let g = crate::player::state::lock_state(&s.player_state);
                    g.queue.shuffle_enabled
                };
                if !shuffle_on
                    && let Err(e) = library::queue::queue_toggle_shuffle(&s)
                {
                    log::warn!("recently_played::shuffle_all toggle: {e}");
                }
            });
        });
    }
}
