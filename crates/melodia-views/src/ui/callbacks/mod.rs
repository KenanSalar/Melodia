//! The wiring that answers to no single view: the `Player` global, `Nav`'s persist and
//! reveal, and what every card grid and dialog shares. Each view slice owns its own under
//! `ui/<view>/callbacks/`.

// `pub(in crate::ui)` because the slices' own `callbacks/` aren't descendants of this module.
pub(in crate::ui) mod macros;

pub(in crate::ui) mod card_actions;
pub(in crate::ui) mod card_selection;
mod clipboard;
pub(in crate::ui) mod cross_tab_nav;
mod dialog_closed;
mod favorite_removal;
pub(in crate::ui) mod grid_tab;
pub(in crate::ui) mod index_persist;
mod library_settings;
mod now_playing;
mod row_flag_mirrors;
mod tags;
pub(in crate::ui) mod track_detail;
pub(in crate::ui) mod track_list;
mod updater;

use std::sync::Arc;

use rand::RngExt;
use slint::{ComponentHandle, Model, ModelRc};

use melodia_app::library;
use melodia_app::services::settings::ViewSort;
use melodia_app::services::view_state::ViewStateData;
use melodia_app::state::AppState;
use melodia_core::entities::track_list::SortDir;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Dialog, Nav, Player};

use index_persist::IndexPersist;
use macros::{spawn_logged, spawn_logged_sync, wire_pb, wire_sync, wire_sync_pb};

pub use cross_tab_nav::wire_cross_tab_nav;
pub(crate) use favorite_removal::FavoriteRemovalUndo;
pub use favorite_removal::wire as wire_favorite_removals;
pub use library_settings::wire_library_settings;
pub use now_playing::{wire_now_playing_favorite, wire_now_playing_rating};
pub use row_flag_mirrors::{RowFlagHandles, RowFlagMirrors};
pub use tags::wire_tags;
pub use updater::{check_painter, wire as wire_updater};

/// A row menu's `[int]` ids as `Vec<i64>`: the one row, or the whole selection.
pub(super) fn collect_track_ids(ids: &ModelRc<i32>) -> Vec<i64> {
    ids.iter().map(i64::from).collect()
}

/// [`collect_track_ids`] less `id == 0`, Browse's disk-only rows, which can't be queued or
/// favorited. `browse/selection.rs` filters a selection already; this guards the single row.
pub(super) fn collect_nonzero_track_ids(ids: &ModelRc<i32>) -> Vec<i64> {
    ids.iter().filter(|&id| id != 0).map(i64::from).collect()
}

/// The queue slot a `play-row` starts on. `idx` indexes the *displayed* rows, the same space
/// as `ids` everywhere but Browse, whose disk-only files `ids` drops, so a mismatch falls back
/// to a lookup by id.
pub(super) fn play_row_start(ids: &[i64], track_id: i64, idx: i32) -> Option<usize> {
    if let Ok(i) = usize::try_from(idx)
        && ids.get(i) == Some(&track_id)
    {
        return Some(i);
    }
    ids.iter().position(|&id| id == track_id)
}

/// Track ids of a live `VecModel<TrackListRow>`, in display order, for Search: its visible set
/// is a sorted, truncated projection built at render time, so the model is the only place that
/// order exists. LIMIT-bounded, so the UI-thread walk is cheap.
pub(super) fn model_track_ids(rows: &ModelRc<melodia_ui::TrackListRow>) -> Vec<i64> {
    rows.iter().map(|r| i64::from(r.id)).collect()
}

/// Which raise intent held the `Dialog` global when a menu started fetching a body for it.
///
/// **Take it synchronously at the click and ask [`Self::holds`] past the `await`**: a body
/// filled after anything else claimed the global paints under another dialog's heading and
/// hands its Accept to the wrong branch. `open` alone can't tell, some openers leaving it false
/// while Rust loads the body or hops a tick, so two menus can both read "nothing is up";
/// `Dialog.request-generation` separates them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct DialogClaim(i32);

impl DialogClaim {
    pub(super) fn take(ui: &AppWindow) -> Self {
        Self(ui.global::<Dialog>().get_request_generation())
    }

    /// [`Self::take`] off a weak handle; `None` where the window has gone.
    pub(super) fn take_from(weak: &slint::Weak<AppWindow>) -> Option<Self> {
        weak.upgrade().as_ref().map(Self::take)
    }

    /// Whether this claim may still fill the global.
    pub(super) fn holds(self, ui: &AppWindow) -> bool {
        !ui.global::<Dialog>().get_open() && Self::take(ui) == self
    }
}

/// Replaces the queue with `ids` from a random slot, then turns shuffle on: the header Shuffle
/// pill. Random because the shuffle anchors the current track at the front, so a head start
/// opens every press on the same song; a set rather than a toggle because the pill means "on",
/// and a toggle racing the transport's button would turn it off.
pub(super) fn spawn_play_then_shuffle(state: &AppState, tag: &'static str, ids: Vec<i64>) {
    // `random_range` panics on an empty range, and a filter matching nothing leaves the pill
    // live, the header gating on the unfiltered count.
    if ids.is_empty() {
        return;
    }
    let start = rand::rng().random_range(..ids.len());
    let state = state.clone();
    state.runtime.clone().spawn(async move {
        if let Err(e) =
            library::playback::player_play_tracks(&state.playback_ctx(), ids, Some(start)).await
        {
            log::warn!("{tag} play: {}", describe(&e));
            return;
        }
        if let Err(e) = library::queue::queue_set_shuffle(&state, true) {
            log::warn!("{tag} shuffle: {}", describe(&e));
        }
    });
}

/// Resolves a sort click: the field already sorted on flips direction, another starts
/// ascending. `cur_dir` is read as [`SortDir::from_token`] reads it, only `"desc"` being
/// descending, so the two can't disagree on a token and strand a pill short of descending.
pub(super) fn next_sort(cur_field: &str, cur_dir: &str, clicked: &str) -> (String, SortDir) {
    next_sort_with_natural(cur_field, cur_dir, clicked, None)
}

/// [`next_sort`] cycling on to `natural` after descending, for a view with an order no header
/// can ask for. Playlist Detail's `"position"` (`TrackDetail::NATURAL_SORT`) gates
/// drag-to-reorder and the sort persists, so without the third state one click on Title would
/// retire reordering. Nothing paints it, no header cell matching the natural field.
pub(super) fn next_sort_with_natural(
    cur_field: &str,
    cur_dir: &str,
    clicked: &str,
    natural: Option<&str>,
) -> (String, SortDir) {
    if cur_field != clicked {
        return (clicked.to_owned(), SortDir::Asc);
    }
    if cur_dir != "desc" {
        return (clicked.to_owned(), SortDir::Desc);
    }
    (natural.unwrap_or(clicked).to_owned(), SortDir::Asc)
}

/// Persists `view_id`'s sort, fire-and-forget: the view already asked for its rows in the new
/// order, so a failure costs only the restart.
pub(super) fn persist_view_sort(
    state: &AppState,
    view_id: &'static str,
    field: String,
    dir: SortDir,
) {
    let s = state.clone();
    let sort = ViewSort { field, dir };
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_view_sort(&s.paths, view_id.to_owned(), sort) {
            log::warn!("{view_id}::set_view_sort: {}", describe(&e));
        }
    });
}

/// `view_id`'s persisted sort, or `None` on a fresh install, where the caller keeps its
/// Slint-declared default. Takes the state `boot` already parsed rather than reading
/// `views.json` once per view ahead of `app.show()`, which could only disagree with it.
pub fn persisted_sort<'a>(
    view_state: Option<&'a ViewStateData>,
    view_id: &str,
) -> Option<&'a ViewSort> {
    view_state?.view_sort.get(view_id)
}

/// Wire every Slint `Player.*` callback to its `library::*` counterpart, and every callback that
/// answers to no single view. Call once after constructing `AppWindow`.
pub fn wire_all(ui: &AppWindow, state: &AppState) {
    card_selection::wire(ui);
    card_actions::wire(ui, state);
    clipboard::wire(ui, state);
    dialog_closed::wire(ui, state);

    let player = ui.global::<Player>();
    wire_transport(&player, state);
    wire_seek(ui, state);
    wire_set_volume(&player, state);
    wire_step_volume(&player, state);
    wire_set_playback_speed(&player, state);
    wire_commit_playback_speed(&player, state);
    // `Player.toggle-favorite` and `FavoriteActions` are wired after every view, so they can
    // reach each one's rows.

    wire_persist_selected_index(ui, state);
    wire_reveal_in_folder(ui, state);
}

fn wire_transport(player: &Player<'_>, state: &AppState) {
    wire_sync_pb!(
        player,
        on_play_pause,
        state,
        "play_pause",
        library::playback::player_toggle_play_pause
    );
    wire_sync_pb!(player, on_next, state, "next", library::playback::player_next);
    wire_sync_pb!(player, on_previous, state, "previous", library::playback::player_previous);
    wire_sync_pb!(player, on_stop, state, "stop", library::playback::player_stop);
    wire_pb!(
        player,
        on_commit_volume,
        state,
        "commit_volume",
        library::playback::commit_player_settings
    );
    wire_pb!(player, on_toggle_mute, state, "toggle_mute", library::playback::player_toggle_mute);
    wire_sync!(
        player,
        on_toggle_shuffle,
        state,
        "toggle_shuffle",
        library::queue::queue_toggle_shuffle
    );
    wire_sync!(player, on_cycle_repeat, state, "cycle_repeat", library::queue::queue_cycle_repeat);
}

/// Hold the slider at the requested position until the backend reports a
/// matching update (`Player.seek_pending_ms`).
fn wire_seek(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    let weak = ui.as_weak();
    ui.global::<Player>().on_seek(move |position_ms| {
        if let Some(ui) = weak.upgrade() {
            ui.global::<Player>().set_seek_pending_ms(position_ms.max(0));
        }
        let s = s.clone();
        let pos = u64::try_from(position_ms.max(0)).unwrap_or(0);
        spawn_logged_sync!(s, "seek", library::playback::player_seek(&s.playback_ctx(), pos));
    });
}

fn wire_set_volume(player: &Player<'_>, state: &AppState) {
    let s = state.clone();
    player.on_set_volume(move |level| {
        let s = s.clone();
        let vol = u32::try_from(level)
            .unwrap_or(0)
            .min(melodia_engine::player::engine::state::MAX_VOLUME);
        spawn_logged_sync!(
            s,
            "set_volume",
            library::playback::player_set_volume(&s.playback_ctx(), vol)
        );
    });
}

fn wire_step_volume(player: &Player<'_>, state: &AppState) {
    let s = state.clone();
    player.on_step_volume(move |delta| {
        let s = s.clone();
        spawn_logged_sync!(
            s,
            "step_volume",
            library::playback::player_step_volume(&s.playback_ctx(), delta)
        );
    });
}

fn wire_set_playback_speed(player: &Player<'_>, state: &AppState) {
    let s = state.clone();
    player.on_set_playback_speed(move |percent| {
        let s = s.clone();
        spawn_logged_sync!(
            s,
            "set_playback_speed",
            library::playback::player_set_playback_speed(
                &s.playback_ctx(),
                speed_from_percent(percent)
            )
        );
    });
}

/// Persists too, speed surviving restarts as repeat, shuffle and volume do.
fn wire_commit_playback_speed(player: &Player<'_>, state: &AppState) {
    let s = state.clone();
    player.on_commit_playback_speed(move |percent| {
        let s = s.clone();
        spawn_logged!(
            s,
            "commit_playback_speed",
            library::playback::player_set_playback_speed_committed(
                &s.playback_ctx(),
                speed_from_percent(percent)
            )
        );
    });
}

/// Whole percents cross from Slint so 1.05 lands as the nearest `f64`; a widened `f32` would
/// persist and print `1.0499999523162842`.
fn speed_from_percent(percent: i32) -> f64 {
    f64::from(percent) / 100.0
}

/// Fired after every sidebar click: records the history entry on the UI thread, off the
/// post-click index and detail id, then persists `last_nav_index` through [`IndexPersist`].
fn wire_persist_selected_index(ui: &AppWindow, state: &AppState) {
    let nav = ui.global::<Nav>();
    let s = state.clone();
    let ui_weak = ui.as_weak();
    let persist = Arc::new(IndexPersist::new(nav.get_selected_index()));
    nav.on_persist_selected_index(move |idx| {
        if let Some(ui) = ui_weak.upgrade() {
            crate::ui::nav_history::record_current(&ui);
        }
        persist.publish(idx);
        // Not `spawn_blocking_logged!`, which takes a literal: a failure names the section.
        let s_disk = s.clone();
        let persist = Arc::clone(&persist);
        s.runtime.spawn_blocking(move || {
            persist.write_if_current(idx, || {
                if let Err(e) = library::settings::set_last_nav_index(&s_disk.paths, idx) {
                    log::warn!("nav: set_last_nav_index({idx}): {}", describe(&e));
                }
            });
        });
    });
}

/// "Open Containing Folder", in every track-row context menu.
fn wire_reveal_in_folder(ui: &AppWindow, state: &AppState) {
    let s = state.clone();
    ui.global::<Nav>().on_reveal_in_folder(move |track_id| {
        let s = s.clone();
        let id = i64::from(track_id);
        s.runtime.clone().spawn(async move {
            if let Err(e) = library::tracks::reveal_in_file_manager(&s.db, id).await {
                log::warn!("nav: reveal_in_folder({id}): {}", describe(&e));
            }
        });
    });
}

#[cfg(test)]
#[path = "tests/play_row_tests.rs"]
mod tests;

// Flat rather than nested under `tests`, where the play-row helpers' `super::` would no
// longer be this file.
#[cfg(test)]
#[path = "tests/index_persist_tests.rs"]
mod index_persist_tests;
