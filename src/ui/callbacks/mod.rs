//! Pull-side glue: every `Player.on_*` callback declared in the Slint UI is
//! wired here to a `library::*` function. Callbacks run on the Slint event
//! loop thread and dispatch the actual work onto the tokio runtime.
//!
//! Split across submodules per Slint global / view domain. `wire_all` is the
//! root entry that wires the `Player` global itself plus the `Nav` persist
//! callback; every other view has its own `wire_*` entrypoint that callers
//! invoke directly (see `boot/ui_setup.rs` or `main.rs`).

mod macros;

mod albums;
mod artists;
mod browse;
mod cross_tab_nav;
mod favorites;
mod genres;
mod library_settings;
mod now_playing;
mod playlists;
mod recently_played;
mod search;
mod tracks;
mod updater;

use slint::{ComponentHandle, Model, ModelRc};

use crate::library;
use crate::state::AppState;
use crate::{AppWindow, Nav, Player};

use macros::{spawn_logged_sync, wire_pb, wire_sync, wire_sync_pb};

#[allow(unused_imports)]
use macros::spawn_logged;

pub use albums::wire_albums;
pub use artists::wire_artists;
pub use browse::wire_browse;
pub use cross_tab_nav::wire_cross_tab_nav;
pub use favorites::wire_favorites;
pub use genres::wire_genres;
pub use library_settings::wire_library_settings;
pub use now_playing::{wire_now_playing_favorite, wire_now_playing_rating};
pub use playlists::{wire_playlist_files, wire_playlists};
pub use recently_played::wire_recently_played;
pub use search::wire_search;
pub use tracks::wire_tracks;
pub use updater::wire as wire_updater;

/// Convert a Slint `[int]` callback param into `Vec<i64>`. Used by every
/// track-list row context-menu callback (`play-next`, `toggle-row-favorite`,
/// `remove-track`) — single-row mode emits a 1-element array, multi-select
/// emits the entire view selection, both feed through here. Centralising
/// the cast keeps `i64::from` (vs `as`) discipline in one place.
pub(super) fn collect_track_ids(ids: &ModelRc<i32>) -> Vec<i64> {
    ids.iter().map(i64::from).collect()
}

/// Variant that drops `id == 0`. Browse view's disk-only rows carry id 0
/// (not in the library, can't be queued/favorited). Selection logic
/// already filters those upstream in `browse/selection.rs`; this is the
/// belt-and-braces guard for the single-row context-menu path.
pub(super) fn collect_nonzero_track_ids(ids: &ModelRc<i32>) -> Vec<i64> {
    ids.iter().filter(|&id| id != 0).map(i64::from).collect()
}

/// Spawn a fire-and-forget task that persists `view_id`'s sort field +
/// direction into `views.json`'s `view_sort`. A write failure is logged, not
/// surfaced — the in-memory re-sort already applied, so the only loss is
/// across a restart. Shared by every sortable view's `on_request_sort`.
pub(super) fn persist_view_sort(state: &AppState, view_id: &'static str, field: String, dir: &str) {
    use crate::services::settings::{SortDir, ViewSort};
    let s = state.clone();
    let sort = ViewSort {
        field,
        dir: SortDir::from_token(dir),
    };
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_view_sort(&s, view_id.to_owned(), sort) {
            log::warn!("{view_id}::set_view_sort: {e}");
        }
    });
}

/// Read `view_id`'s persisted sort as `(field, dir)` display strings, or
/// `None` when the view never persisted one (fresh install — caller keeps
/// its Slint-global default). Counterpart of [`persist_view_sort`] used by
/// each view's `wire_*` to seed the sort header at startup.
pub(super) fn persisted_sort(state: &AppState, view_id: &str) -> Option<(String, &'static str)> {
    library::settings::get_view_sort(state, view_id).map(|s| (s.field, s.dir.as_str()))
}

/// Wire every Slint `Player.*` callback to its `library::*` counterpart.
/// Call once after constructing `AppWindow`.
pub fn wire_all(ui: &AppWindow, state: &AppState) {
    let player = ui.global::<Player>();
    let ui_weak = ui.as_weak();

    wire_sync_pb!(player, on_play_pause, state, "play_pause", library::playback::player_toggle_play_pause);
    wire_sync_pb!(player, on_next, state, "next", library::playback::player_next);
    wire_sync_pb!(player, on_previous, state, "previous", library::playback::player_previous);
    wire_pb!(player, on_commit_volume, state, "commit_volume", library::playback::commit_player_settings);
    wire_pb!(player, on_toggle_mute, state, "toggle_mute", library::playback::player_toggle_mute);
    wire_sync!(player, on_toggle_shuffle, state, "toggle_shuffle", library::queue::queue_toggle_shuffle);
    wire_sync!(player, on_cycle_repeat, state, "cycle_repeat", library::queue::queue_cycle_repeat);

    // seek: hold the slider at the requested position until the backend
    // reports a matching update (see Player.seek_pending_ms).
    {
        let s = state.clone();
        let weak = ui_weak.clone();
        player.on_seek(move |position_ms| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<Player>().set_seek_pending_ms(position_ms.max(0));
            }
            let s = s.clone();
            let pos = u64::try_from(position_ms.max(0)).unwrap_or(0);
            spawn_logged_sync!(s, "seek", library::playback::player_seek(&s.playback_ctx(), pos));
        });
    }

    // set_volume: clamp + cast before dispatch.
    {
        let s = state.clone();
        player.on_set_volume(move |level| {
            let s = s.clone();
            let vol = u32::try_from(level.clamp(0, 200)).unwrap_or(0);
            spawn_logged_sync!(s, "set_volume", library::playback::player_set_volume(&s.playback_ctx(), vol));
        });
    }

    // set_playback_speed: apply to the live player AND persist (speed
    // survives restarts — mirrors repeat/shuffle/volume). The flyout only
    // ever sends valid preset values; downstream clamps anyway, so no
    // clamp is needed here. Two steps like the gapless callback in
    // `src/ui/playback_settings.rs`: (a) fast synchronous runtime apply,
    // (b) blocking-pool disk write.
    {
        let s = state.clone();
        player.on_set_playback_speed(move |speed| {
            let speed = f64::from(speed);
            let s_apply = s.clone();
            spawn_logged_sync!(
                s_apply,
                "set_playback_speed",
                library::playback::player_set_playback_speed(&s_apply.playback_ctx(), speed)
            );
            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_playback_speed(&s_disk, speed) {
                    log::warn!("persist playback_speed: {e}");
                }
            });
        });
    }

    // Player.toggle-favorite is wired in `wire_now_playing_favorite` (called
    // after every per-view wire fn) so it can fan the change into all three
    // surfaces that hold a per-row `is_favorite` (Tracks, Browse, AlbumDetail).

    // Nav.persist-selected-index: fired by the sidebar TouchArea after every
    // tab click. Persist into `views.json`'s `last_nav_index` on the blocking pool
    // so a restart lands on the same section, and record a browser-style
    // history entry so Mouse-4/Mouse-5 can walk back/forward through tab
    // switches. The `record_current` read happens on the UI thread before
    // any disk hop; reads the post-click `Nav.selected-index` + the
    // section's current detail-id (if any), so the entry reflects what
    // the user is actually about to see.
    let nav = ui.global::<Nav>();
    {
        let s = state.clone();
        let ui_weak = ui_weak.clone();
        nav.on_persist_selected_index(move |idx| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::ui::nav_history::record_current(&s, &ui);
            }
            let s_disk = s.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) = library::settings::set_last_nav_index(&s_disk, idx) {
                    log::warn!("nav: set_last_nav_index({idx}): {e}");
                }
            });
        });
    }

    // Nav.reveal-in-folder: fired by the "Open Containing Folder" entry
    // in every track-row context menu. Resolves the track's file path
    // and opens its parent directory in the OS file manager.
    {
        let s = state.clone();
        nav.on_reveal_in_folder(move |track_id| {
            let s = s.clone();
            let id = i64::from(track_id);
            s.runtime.clone().spawn(async move {
                if let Err(e) = library::tracks::reveal_in_file_manager(&s, id).await {
                    log::warn!("nav: reveal_in_folder({id}): {e}");
                }
            });
        });
    }
}
