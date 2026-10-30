//! The results' callbacks: the cross-tab opens, Top Result routing and the Songs list.

use std::collections::HashSet;
use std::sync::Arc;

use slint::{ComponentHandle, Global as _, SharedString};

use super::NAV_SEARCH;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::cross_tab_nav;
use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{
    play_displayed, wire_queue_actions, wire_row_selection, wire_toggle_column,
};
use crate::ui::callbacks::{collect_track_ids, model_track_ids, next_sort, persist_view_sort};
use crate::ui::search::{SearchUi, apply, fetch};
use crate::ui::track_list_view::view_id;
use melodia_app::library;
use melodia_app::services::settings::ViewSort;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Nav, Search};

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
    // Ranges over the displayed rows, so a shift-click follows the compact/full toggle and never
    // reaches the rows "Show all" hides.
    wire_row_selection(&g.as_weak());
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

/// Routes through the kind's own open callback, so the origin stamp and the nav move stay in one
/// place.
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
            "genre" => ui.global::<Nav>().invoke_go_to_genre(id),
            _ => {}
        }
    });
}

/// Queues the visible results, starting on the clicked track. The ids come off the live model,
/// the one place the sort and the compact cut both hold.
fn wire_play_row(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Search>().on_play_row(move |track_id, idx| {
        let Some(ui) = weak.upgrade() else { return };
        let ids = model_track_ids(&ui.global::<Search>().get_tracks());
        play_displayed(&s, view_id::SEARCH, ids, track_id, idx);
    });
}

/// Patched by hand, Search having no `library_changed` subscriber: unpatched, the pip never moves
/// and the next click re-sends the value the database already holds.
fn wire_row_flags(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();
    {
        let su = search_ui.clone();
        wire_row_flag!(g, on_toggle_row_favorite, state, "search::set_favorite",
        library::favorites::set_favorite, collect_track_ids,
        captures: [weak, su],
        after: |id_vec, fav| {
            let ids: HashSet<i64> = id_vec.into_iter().collect();
            su.flip_favorites(&ids, fav);
            apply::apply_rows_favorite(&weak, ids, fav);
        });
    }
    {
        let su = search_ui.clone();
        wire_row_flag!(g, on_set_row_rating, state, "search::set_rating",
        library::ratings::set_rating, collect_track_ids,
        captures: [weak, su],
        after: |id_vec, rating| {
            let ids: HashSet<i64> = id_vec.into_iter().collect();
            su.flip_ratings(&ids, rating);
            apply::apply_rows_rating(&weak, ids, rating);
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

        fetch::reapply_cached_results(&su, &weak);
    });
}
