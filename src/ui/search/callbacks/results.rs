//! `Search.*` result callbacks: cross-tab open-album / open-artist
//! hand-offs, Top Result routing, `TrackList` row actions (play, queue,
//! favorite toggle), sort, column visibility, and selection. See
//! [`super::wire`].

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use super::NAV_SEARCH;
use crate::library;
use crate::services::settings::ViewSort;
use crate::state::AppState;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::cross_tab_nav;
use crate::ui::callbacks::macros::{spawn_blocking_logged, spawn_logged};
use crate::ui::callbacks::{
    collect_track_ids, model_track_ids, next_sort, persist_view_sort, play_row_start,
};
use crate::ui::search::{self as search_ui_mod, SearchUi, fetch};
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::{AppWindow, Search};

/// Wire the cross-tab open / Top Result / row-action / sort / selection
/// callbacks.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    search_ui: &Arc<SearchUi>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // --- Cross-tab open-album --------------------------------------
    {
        let s = state.clone();
        let au = albums_ui.clone();
        let weak = weak.clone();
        g.on_open_album(move |album_id| {
            cross_tab_nav::open_album_cross_tab(
                &s,
                &au,
                &weak,
                i64::from(album_id),
                cross_tab_nav::Origin::section(NAV_SEARCH),
                "search::open_album",
            );
        });
    }

    // --- Cross-tab open-artist -------------------------------------
    {
        let s = state.clone();
        let aru = artists_ui.clone();
        let weak = weak.clone();
        g.on_open_artist(move |artist_id| {
            cross_tab_nav::open_artist_cross_tab(
                &s,
                &aru,
                &weak,
                i64::from(artist_id),
                cross_tab_nav::Origin::section(NAV_SEARCH),
                "search::open_artist",
            );
        });
    }

    // --- Top Result click ------------------------------------------
    // Resolves to one of three cross-tab handlers based on `top-kind` —
    // the two above plus `go-to-genre`, wired in `cross_tab_nav`.
    // Invoking the global callback directly keeps the origin-stamp +
    // nav-flip + persist all in one place.
    {
        let weak = weak.clone();
        g.on_open_top_result(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Search>();
            let kind = g.get_top_kind();
            let id = g.get_top_id();
            if id < 0 {
                return;
            }
            match kind.as_str() {
                "album" => g.invoke_open_album(id),
                "artist" => g.invoke_open_artist(id),
                "genre" => g.invoke_go_to_genre(id),
                _ => {}
            }
        });
    }

    // --- Songs row actions -----------------------------------------
    // play-row loads the visible results into the queue and starts on the
    // clicked track. Search keeps no Rust-side cache of what's on screen (the
    // sort and the `show-all-tracks` cap are applied at render), so the ids
    // come off the live model.
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_play_row(move |track_id, idx| {
            let Some(ui) = weak.upgrade() else { return };
            let ids = model_track_ids(&ui.global::<Search>().get_tracks());
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(s, "search::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }
    {
        let s = state.clone();
        g.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "search::play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }
    {
        let s = state.clone();
        g.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(s, "search::add_to_queue",
                library::queue::queue_add_tracks(&s, id_vec));
        });
    }
    {
        let s = state.clone();
        g.on_toggle_row_favorite(move |ids, fav| {
            let id_vec = collect_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::favorites::set_favorite(&s, id_vec, fav).await {
                    log::warn!("search::set_favorite: {e}");
                }
                // No optimistic local update — `Search.tracks` is
                // backed by `last_results` which lives on disk; the
                // next `library_changed_tx` tick won't refresh us
                // (Search is query-driven), and a stale `is_favorite`
                // pip on a result row resolves itself on the user's
                // next search or page revisit. Better than mutating
                // the cached result set behind the user's back.
            });
        });
    }
    {
        let s = state.clone();
        g.on_set_row_rating(move |ids, rating| {
            let id_vec = collect_track_ids(&ids);
            if id_vec.is_empty() {
                return;
            }
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::ratings::set_rating(&s, id_vec, rating).await {
                    log::warn!("search::set_rating: {e}");
                }
                // No optimistic local update — mirrors `on_toggle_row_favorite`
                // above (Search is query-driven; the hover star reflects the
                // new rating on the user's next search or page revisit).
            });
        });
    }
    {
        let s = state.clone();
        let su = search_ui.clone();
        let weak = weak.clone();
        g.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Search>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            *su.state().sort.lock() = ViewSort {
                field: new_field.clone(),
                dir: new_dir,
            };
            persist_view_sort(&s, view_id::SEARCH, new_field, new_dir);

            // Re-derive the visible Songs slice from the cached
            // results — no DB hit. If there are no cached results
            // yet (no commit ran since startup) this is a no-op.
            fetch::swap_tracks_compact_or_full(&su, &weak);
        });
    }
    {
        let s = state.clone();
        let weak = weak.clone();
        g.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<Search>().snapshot_visible();
            let s_disk = s.clone();
            spawn_blocking_logged!(s, "search::toggle_column",
                library::settings::update_view_columns(&s_disk, "search".to_owned(), columns));
        });
    }
    {
        let weak = weak.clone();
        let su = search_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            search_ui_mod::handle_select_row(&ui, &su, idx, id, shift, ctrl);
        });
    }
    {
        let weak = weak.clone();
        let su = search_ui.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            search_ui_mod::clear_selection(&ui, &su);
        });
    }
}
