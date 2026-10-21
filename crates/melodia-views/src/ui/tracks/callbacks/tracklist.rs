//! The Songs list itself: sort, filter, play, queue, favorite, rating, selection, columns.

use std::sync::Arc;

use slint::{ComponentHandle, Global as _, SharedString};

use crate::ui::callbacks::macros::wire_row_flag;
use crate::ui::callbacks::track_list::{
    play_displayed, wire_queue_actions, wire_row_selection, wire_toggle_column,
};
use crate::ui::callbacks::{collect_track_ids, next_sort, persist_view_sort, persisted_sort};
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::track_list_view::view_id;
use crate::ui::tracks::{self as tracks_ui_mod, SongsQuery, TracksUi};
use melodia_app::library;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Tracks};

/// Wire the list's own callbacks, and seed the sort header they persist through.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    view_state: Option<&ViewStateData>,
    tracks_ui: &Arc<TracksUi>,
) {
    let tracks = ui.global::<Tracks>();
    seed_sort(&tracks, view_state);
    wire_sort(ui, state, tracks_ui);
    wire_filter(ui, state, tracks_ui);
    wire_play_row(ui, state);
    wire_queue_actions(&tracks, state, collect_track_ids);
    wire_row_flags(ui, state, tracks_ui);
    wire_row_selection(&tracks.as_weak());
    wire_toggle_column(&tracks.as_weak(), state);
    wire_refresh(ui, state, tracks_ui);
}

/// Seed the sort header from the persisted `view_sort["tracks"]` so the
/// arrow matches the order `spawn_initial_tracks_fetch` fetches with.
fn seed_sort(tracks: &Tracks<'_>, view_state: Option<&ViewStateData>) {
    if let Some(sort) = persisted_sort(view_state, view_id::TRACKS) {
        tracks.set_sort_field(SharedString::from(sort.field.as_str()));
        tracks.set_sort_dir(SharedString::from(sort.dir.as_str()));
    }
}

/// Clicking a header column. Same field flips dir; new field resets to
/// ascending. The ids reload in the new order and every cached row is kept;
/// the sort is persisted so it survives a restart.
fn wire_sort(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let s = state.clone();
    let tu = tracks_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_request_sort(move |field| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Tracks>();
        let (new_field, new_dir) =
            next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
        g.set_sort_field(SharedString::from(new_field.as_str()));
        g.set_sort_dir(SharedString::from(new_dir.as_str()));

        let query = SongsQuery::new(&new_field, new_dir.as_str(), &g.get_filter());
        tracks_ui_mod::requery(&s, &tu, query);
        persist_view_sort(&s, view_id::TRACKS, new_field, new_dir);
    });
}

fn wire_filter(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let s = state.clone();
    let tu = tracks_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_apply_filter(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Tracks>();
        let query = SongsQuery::new(&g.get_sort_field(), &g.get_sort_dir(), &text);
        tracks_ui_mod::requery(&s, &tu, query);
    });
}

/// Double-click loads the current view into the queue and starts
/// on the clicked track — the standard music-player contract.
fn wire_play_row(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_play_row(move |track_id, idx| {
        let Some(ui) = weak.upgrade() else { return };
        let ids = TrackRows::ids_of(&ui.global::<Tracks>().get_rows(), "Tracks.rows");
        play_displayed(&s, view_id::TRACKS, ids, track_id, idx);
    });
}

/// Write through, then patch each affected row (no list re-fetch, so
/// scroll position holds and there's no flash). Rating never changes list
/// membership.
fn wire_row_flags(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let tracks = ui.global::<Tracks>();
    let tu = tracks_ui.clone();
    wire_row_flag!(tracks, on_toggle_row_favorite, state, "tracks::set_favorite",
    library::favorites::set_favorite, collect_track_ids,
    captures: [tu],
    after: |id_vec, fav| {
        for id in id_vec {
            track_rows::patch_row(&*tu, id, move |row| row.is_favorite = fav);
        }
    });
    wire_row_flag!(tracks, on_set_row_rating, state, "tracks::set_rating",
    library::ratings::set_rating, collect_track_ids,
    captures: [tu],
    after: |id_vec, rating| {
        for id in id_vec {
            track_rows::patch_row(&*tu, id, move |row| row.rating = rating);
        }
    });
}

/// Reload with the current sort and filter: a library tick while the tab is
/// up, or a re-enter after one landed while it wasn't.
fn wire_refresh(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let s = state.clone();
    let tu = tracks_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_request_refresh(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Tracks>();
        let query = SongsQuery::new(&g.get_sort_field(), &g.get_sort_dir(), &g.get_filter());
        tracks_ui_mod::refresh(&s, &tu, query);
    });
}
