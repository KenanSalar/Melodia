//! The Songs list itself: sort, filter, play, queue, favorite, rating, selection, columns.

use std::sync::Arc;

use slint::{ComponentHandle, Global as _, SharedString};

use crate::ui::callbacks::macros::{spawn_logged, wire_row_flag};
use crate::ui::callbacks::track_list::{play_displayed, wire_queue_actions, wire_toggle_column};
use crate::ui::callbacks::{collect_track_ids, next_sort, persist_view_sort, persisted_sort};
use crate::ui::track_list_view::view_id;
use crate::ui::tracks::{self as tracks_ui_mod, TracksUi};
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
    wire_filter(ui, tracks_ui);
    wire_play_row(ui, state, tracks_ui);
    wire_queue_actions(&tracks, state, collect_track_ids);
    wire_row_flags(ui, state, tracks_ui);
    wire_selection(ui, tracks_ui);
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

/// Clicking a header column. Same field flips dir; new field
/// resets to ascending. Re-sort is done entirely in memory — no DB
/// re-fetch and no `RowSearchKey` rebuild (`resort_and_apply`); the new
/// sort is persisted so it survives a restart.
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
        let filter = g.get_filter();

        tracks_ui_mod::resort_and_apply(&weak, &tu, &new_field, new_dir.as_str(), &filter);
        persist_view_sort(&s, view_id::TRACKS, new_field, new_dir);
    });
}

/// Client-side; no DB hit.
fn wire_filter(ui: &AppWindow, tracks_ui: &Arc<TracksUi>) {
    let tu = tracks_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_apply_filter(move |text| {
        tracks_ui_mod::refilter(&weak, &tu, &text);
    });
}

/// Double-click loads the current view into the queue and starts
/// on the clicked track — the standard music-player contract.
fn wire_play_row(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let s = state.clone();
    let tu = tracks_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_play_row(move |track_id, idx| {
        let Some(ui) = weak.upgrade() else { return };
        let filter = ui.global::<Tracks>().get_filter().to_string();
        play_displayed(&s, view_id::TRACKS, tu.current_ids_filtered(&filter), track_id, idx);
    });
}

/// Write through, then surgically update each affected row (no list
/// re-fetch, so scroll position holds and there's no flash). Rating never
/// changes list membership.
fn wire_row_flags(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let tracks = ui.global::<Tracks>();
    let weak = ui.as_weak();
    {
        let tu = tracks_ui.clone();
        wire_row_flag!(tracks, on_toggle_row_favorite, state, "tracks::set_favorite",
        library::favorites::set_favorite, collect_track_ids,
        captures: [weak, tu],
        after: |id_vec, fav| {
            for id in &id_vec {
                tu.flip_favorite(*id, fav);
                tracks_ui_mod::apply_row_favorite(&weak, *id, fav);
            }
        });
    }
    {
        let tu = tracks_ui.clone();
        wire_row_flag!(tracks, on_set_row_rating, state, "tracks::set_rating",
        library::ratings::set_rating, collect_track_ids,
        captures: [weak, tu],
        after: |id_vec, rating| {
            for id in &id_vec {
                tu.flip_rating(*id, rating);
                tracks_ui_mod::apply_row_rating(&weak, *id, rating);
            }
        });
    }
}

/// select-row: modifier-aware click handler. Computes the new selected
/// set in Rust (Slint expressions can't iterate a model for a membership
/// check) and walks the visible `VecModel` to flip per-row `selected` flags.
fn wire_selection(ui: &AppWindow, tracks_ui: &Arc<TracksUi>) {
    let tracks = ui.global::<Tracks>();
    {
        let weak = ui.as_weak();
        let tu = tracks_ui.clone();
        tracks.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            tracks_ui_mod::handle_select_row(&ui, &tu, idx, id, shift, ctrl);
        });
    }
    {
        let weak = ui.as_weak();
        tracks.on_select_all(move || {
            let Some(ui) = weak.upgrade() else { return };
            tracks_ui_mod::select_all(&ui);
        });
    }
    {
        let weak = ui.as_weak();
        tracks.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            tracks_ui_mod::clear_selection(&ui);
        });
    }
}

/// Re-fetch with current sort + filter (e.g. after a scan
/// completes — wired up later when scan-complete events surface).
fn wire_refresh(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let s = state.clone();
    let tu = tracks_ui.clone();
    let weak = ui.as_weak();
    ui.global::<Tracks>().on_request_refresh(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Tracks>();
        let field = g.get_sort_field().to_string();
        let dir = g.get_sort_dir().to_string();
        let filter = g.get_filter().to_string();
        let s = s.clone();
        let tu = tu.clone();
        let weak = weak.clone();
        spawn_logged!(
            s,
            "tracks::refresh",
            tracks_ui_mod::fetch_and_apply(&s, &tu, weak, field, dir, filter)
        );
    });
}
