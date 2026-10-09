//! `Search.*` result callbacks: cross-tab open-album / open-artist
//! hand-offs, Top Result routing, `TrackList` row actions (play, queue,
//! favorite toggle), sort, column visibility, and selection. See
//! [`super::wire`].

use std::sync::Arc;

use slint::{ComponentHandle, Global as _, SharedString};

use super::NAV_SEARCH;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::cross_tab_nav;
use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{play_displayed, wire_queue_actions, wire_toggle_column};
use crate::ui::callbacks::{collect_track_ids, model_track_ids, next_sort, persist_view_sort};
use crate::ui::search::{self as search_ui_mod, SearchUi, apply, fetch};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::services::settings::ViewSort;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Search};

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
    wire_open_album(ui, state, albums_ui);
    wire_open_artist(ui, state, artists_ui);
    wire_open_top_result(ui);
    wire_play_row(ui, state);
    wire_queue_actions(&g, state, collect_track_ids);
    wire_row_flags(ui, state, search_ui);
    wire_sort(ui, state, search_ui);
    wire_toggle_column(&g.as_weak(), state);
    wire_selection(ui, search_ui);
}

fn wire_open_album(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    let s = state.clone();
    let au = albums_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Search>().on_open_album(move |album_id| {
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

fn wire_open_artist(ui: &AppWindow, state: &AppState, artists_ui: &Arc<ArtistsUi>) {
    let s = state.clone();
    let aru = artists_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Search>().on_open_artist(move |artist_id| {
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

/// Resolves to one of three cross-tab handlers based on `top-kind` —
/// the two above plus `go-to-genre`, wired in `cross_tab_nav`.
/// Invoking the global callback directly keeps the origin-stamp +
/// nav-flip + persist all in one place.
fn wire_open_top_result(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<Search>().on_open_top_result(move || {
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

/// Loads the visible results into the queue and starts on the
/// clicked track. Search keeps no Rust-side cache of what's on screen (the
/// sort and the `show-all-tracks` cap are applied at render), so the ids
/// come off the live model.
fn wire_play_row(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Search>().on_play_row(move |track_id, idx| {
        let Some(ui) = weak.upgrade() else { return };
        let ids = model_track_ids(&ui.global::<Search>().get_tracks());
        play_displayed(&s, view_id::SEARCH, ids, track_id, idx);
    });
}

/// Write through, then patch the cached result set
/// and the visible row. Search has no `library_changed` subscriber to fall back on, so
/// without the patch the pip never moves and the next click recomputes the same `!false`
/// and re-sends a value the database already holds.
fn wire_row_flags(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();
    {
        let su = search_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "search::set_favorite",
        library::favorites::set_favorite, collect_track_ids,
        captures: [weak, su],
        after: |id_vec, fav| {
            for id in &id_vec {
                su.flip_favorite(*id, fav);
                apply::apply_row_favorite(&weak, *id, fav);
            }
        });
    }
    {
        let su = search_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "search::set_rating",
        library::ratings::set_rating, collect_track_ids,
        captures: [weak, su],
        after: |id_vec, rating| {
            for id in &id_vec {
                su.flip_rating(*id, rating);
                apply::apply_row_rating(&weak, *id, rating);
            }
        });
    }
}

fn wire_sort(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let s = state.clone();
    let su = search_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Search>().on_request_sort(move |field| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Search>();
        let (new_field, new_dir) =
            next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
        g.set_sort_field(SharedString::from(new_field.as_str()));
        g.set_sort_dir(SharedString::from(new_dir.as_str()));
        *su.state().sort.lock() = ViewSort { field: new_field.clone(), dir: new_dir };
        persist_view_sort(&s, view_id::SEARCH, new_field, new_dir);

        // Re-derive the visible Songs slice from the cached
        // results — no DB hit. If there are no cached results
        // yet (no commit ran since startup) this is a no-op.
        fetch::reapply_cached_results(&su, &weak);
    });
}

fn wire_selection(ui: &AppWindow, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    {
        let weak = ui.as_weak();
        let su = search_ui.clone();
        g.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            search_ui_mod::handle_select_row(&ui, &su, idx, id, shift, ctrl);
        });
    }
    {
        let weak = ui.as_weak();
        let su = search_ui.clone();
        g.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            search_ui_mod::select_all(&ui, &su);
        });
    }
    {
        let weak = ui.as_weak();
        let su = search_ui.clone();
        g.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            search_ui_mod::clear_selection(&ui, &su);
        });
    }
}
