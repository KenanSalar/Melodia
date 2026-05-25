//! `Tracks.*` callbacks: sort, filter, play, favorite, selection,
//! column-visibility persistence.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use super::collect_track_ids;
use super::macros::{spawn_logged, spawn_logged_sync};
use crate::library;
use crate::state::AppState;
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::ui::tracks::{self as tracks_ui_mod, TracksUi};
use crate::{AppWindow, Tracks};

/// Wire every `Tracks.*` callback to its `library::*` counterpart and the
/// `tracks_ui` shared state. Call once after `wire_all` and after
/// `tracks::install_tracks_model`.
pub fn wire_tracks(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let tracks = ui.global::<Tracks>();
    let weak = ui.as_weak();

    // Seed the sort header from the persisted `view_sort["tracks"]` so the
    // arrow matches the order `spawn_initial_tracks_fetch` fetches with.
    if let Some((field, dir)) = super::persisted_sort(state, view_id::TRACKS) {
        tracks.set_sort_field(SharedString::from(field.as_str()));
        tracks.set_sort_dir(SharedString::from(dir));
    }

    // request-sort: clicking a header column. Same field flips dir; new field
    // resets to ascending. Re-sort is done entirely in memory — no DB
    // re-fetch and no `RowSearchKey` rebuild (`resort_and_apply`); the new
    // sort is persisted so it survives a restart.
    {
        let s = state.clone();
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Tracks>();
            let cur_field = g.get_sort_field();
            let cur_dir = g.get_sort_dir();
            let (new_field, new_dir) = if cur_field.as_str() == field.as_str() {
                let nd = if cur_dir.as_str() == "asc" { "desc" } else { "asc" };
                (field.to_string(), nd.to_string())
            } else {
                (field.to_string(), "asc".to_string())
            };
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            let filter = g.get_filter().to_string();

            tracks_ui_mod::resort_and_apply(&weak, &tu, &new_field, &new_dir, filter);
            super::persist_view_sort(&s, view_id::TRACKS, new_field, &new_dir);
        });
    }

    // apply-filter: client-side; no DB hit.
    {
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_apply_filter(move |text| {
            tracks_ui_mod::refilter(&weak, &tu, text.to_string());
        });
    }

    // play-row: double-click on a row appends just that track to the queue
    // (skipping if already present). Auto-starts when the player was idle.
    // The "Play All" button is the explicit affordance for loading the full
    // filtered list — see `on_play_all` below.
    {
        let s = state.clone();
        tracks.on_play_row(move |track_id, _idx| {
            let s = s.clone();
            let id = i64::from(track_id);
            spawn_logged!(s, "tracks::play_row",
                library::queue::queue_append_unique(&s, id));
        });
    }

    // play-all: load every track in the current filter into the queue and
    // start at index 0. Matches the explicit "replace queue + play"
    // semantics of Album/Artist/Genre/Browse Play-All buttons.
    {
        let s = state.clone();
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_play_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            let filter = ui.global::<Tracks>().get_filter().to_string();
            let ids = tu.current_ids_filtered(&filter);
            if ids.is_empty() {
                return;
            }
            let s = s.clone();
            spawn_logged!(s, "tracks::play_all",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)));
        });
    }

    {
        let s = state.clone();
        tracks.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        tracks.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(s, "add_to_queue", library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite: write through, then surgically update each
    // affected row (no list re-fetch, so scroll position holds and
    // there's no flash). The Slint side passes an `[int]` so a single-
    // row click and a multi-select batch share one code path.
    {
        let s = state.clone();
        let weak = weak.clone();
        let tu = tracks_ui.clone();
        tracks.on_toggle_row_favorite(move |ids, fav| {
            let id_vec = collect_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            let weak = weak.clone();
            let tu = tu.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::favorites::set_favorite(&s, id_vec.clone(), fav).await {
                    log::warn!("set_favorite: {e}");
                    return;
                }
                for id in &id_vec {
                    tu.flip_favorite(*id, fav);
                    tracks_ui_mod::apply_row_favorite(&weak, *id, fav);
                }
            });
        });
    }

    // select-row: modifier-aware click handler. Computes the new selected
    // set in Rust (Slint expressions can't iterate a model for a membership
    // check) and walks the visible VecModel to flip per-row `selected` flags.
    {
        let weak = weak.clone();
        let tu = tracks_ui.clone();
        tracks.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            tracks_ui_mod::handle_select_row(&ui, &tu, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        tracks.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            tracks_ui_mod::clear_selection(&ui);
        });
    }

    // request-refresh: re-fetch with current sort + filter (e.g. after a scan
    // completes — wired up later when scan-complete events surface).
    {
        let s = state.clone();
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_request_refresh(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Tracks>();
            let field = g.get_sort_field().to_string();
            let dir = g.get_sort_dir().to_string();
            let filter = g.get_filter().to_string();
            let s = s.clone();
            let tu = tu.clone();
            let weak = weak.clone();
            spawn_logged!(s, "tracks::refresh",
                tracks_ui_mod::fetch_and_apply(&s, &tu, weak, field, dir, filter));
        });
    }

    // toggle-column: the popup has already flipped the matching `show-*`
    // flag for instant visual feedback. We persist the new visible-column
    // list to `views.json`'s `view_columns["tracks"]` so the choice survives
    // restarts. The Slint flag is the source of truth here — we read it
    // through `TrackListColumnState::snapshot_visible` so the on-disk shape
    // stays consistent with the shutdown-snapshot path.
    {
        let s = state.clone();
        let weak = weak.clone();
        tracks.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<Tracks>().snapshot_visible();
            let s = s.clone();
            spawn_logged_sync!(s, "tracks::toggle_column",
                library::settings::update_view_columns(&s, "tracks".to_string(), columns));
        });
    }
}
