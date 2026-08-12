//! Pull-side glue: every `Player.on_*` callback declared in the Slint UI is
//! wired here to a `library::*` function. Callbacks run on the Slint event
//! loop thread and dispatch the actual work onto the tokio runtime.
//!
//! What is left here is the cross-cutting set — everything answering to no
//! single view. `wire_all` is the root entry that wires the `Player` global
//! itself plus the `Nav` persist callback; each view slice owns its own wiring
//! under `ui/<view>/callbacks/` and reaches it through that slice's `install`.

// `pub(in crate::ui)` rather than private: each view slice owns its own
// `callbacks/` submodule, so the wiring reaching these is no longer a
// descendant of this module. Still sealed inside `ui::` — nothing in
// `library/`, `player/` or `boot/` can name either.
pub(in crate::ui) mod macros;

pub(in crate::ui) mod cross_tab_nav;
mod library_settings;
mod now_playing;
mod tags;
mod updater;

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use rand::RngExt;
use slint::{ComponentHandle, Model, ModelRc};

use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::{AppWindow, Nav, Player};

use macros::{spawn_logged_sync, wire_pb, wire_sync, wire_sync_pb};

pub use cross_tab_nav::wire_cross_tab_nav;
pub use library_settings::wire_library_settings;
pub use now_playing::{wire_now_playing_favorite, wire_now_playing_rating};
pub use tags::wire_tags;
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

/// Resolve the queue slot a `play-row` activation should start on.
///
/// The row index a view hands over indexes its *displayed* rows, which is the
/// same space as `ids` everywhere except Browse — that list also shows
/// disk-only files, which `current_in_library_ids` drops. Fall back to a lookup
/// by id there, and to the head of the queue when the track isn't in the list
/// at all (`player_play_tracks` reads `None` as index 0).
pub(super) fn play_row_start(ids: &[i64], track_id: i64, idx: i32) -> Option<usize> {
    if let Ok(i) = usize::try_from(idx)
        && ids.get(i) == Some(&track_id)
    {
        return Some(i);
    }
    ids.iter().position(|&id| id == track_id)
}

/// Track ids of a live `VecModel<TrackListRow>`, in display order.
///
/// Only Search needs this. Every other view caches its displayed rows on a
/// `*Ui` handle, but Search's visible set is a sorted, `COMPACT_TRACK_LIMIT`-
/// truncated projection of `last_results` that is assembled at render time —
/// so the model is the only place the displayed order actually exists. Result
/// sets are LIMIT-bounded, so walking it on the UI thread is cheap.
pub(super) fn model_track_ids(rows: &ModelRc<crate::TrackListRow>) -> Vec<i64> {
    rows.iter().map(|r| i64::from(r.id)).collect()
}

/// Replace the queue with `ids`, open on a random one, then turn shuffle on —
/// the header Shuffle pill on the six views that carry one. `tag` names the
/// call site in the two failure logs.
///
/// The two halves have to be sequenced (shuffle re-anchors around whatever is
/// playing), so this is a spawned task rather than one `library::*` call. Use
/// `queue_set_shuffle` and not a read-then-toggle pair: the pill means "on",
/// and a toggle racing the transport's shuffle button would turn it off.
///
/// The start slot is random, and has to be: the shuffle anchors the current
/// track at the front, so starting at the head would open every press on the
/// same song — a shuffle whose first track you can predict.
pub(super) fn spawn_play_then_shuffle(state: &AppState, tag: &'static str, ids: Vec<i64>) {
    // Not just an early exit — `random_range` panics on an empty range, and a
    // filter that matches nothing leaves a live pill over an empty list (the
    // header gates on the view's *unfiltered* count).
    if ids.is_empty() {
        return;
    }
    let start = rand::rng().random_range(..ids.len());
    let state = state.clone();
    state.runtime.clone().spawn(async move {
        if let Err(e) =
            library::playback::player_play_tracks(&state.playback_ctx(), ids, Some(start)).await
        {
            log::warn!("{tag} play: {e}");
            return;
        }
        if let Err(e) = library::queue::queue_set_shuffle(&state, true) {
            log::warn!("{tag} shuffle: {e}");
        }
    });
}

/// Resolve a sort-pill (or column-header) click against the sort in force.
///
/// Clicking the field already sorted on flips the direction; clicking a
/// different one starts it ascending. Every sortable view spelled this out
/// identically, so a change of mind about the reset direction meant finding
/// eleven copies.
///
/// `cur_dir` is read the way [`SortDir::from_token`] reads it — only `"desc"`
/// is descending — rather than testing for `"asc"` and treating everything else
/// as descending. The two only differ on a token neither side can currently
/// produce, but disagreeing about it would leave that pill unable to reach
/// descending at all.
pub(super) fn next_sort(cur_field: &str, cur_dir: &str, clicked: &str) -> (String, SortDir) {
    next_sort_with_natural(cur_field, cur_dir, clicked, None)
}

/// [`next_sort`] for a view with an order of its own to fall back to:
/// ascending, descending, then `natural`.
///
/// A *synthetic* order — one no column header can ask for — is otherwise
/// unreachable the moment anything else is clicked, and the sort persists, so
/// it stays unreachable across restarts. Playlist Detail is the only caller:
/// `"position"` is what drag-to-reorder is gated on, so one click on Title used
/// to retire reordering for good. Everything else passes `None`.
///
/// Nothing paints the third state — with the natural field in force no header
/// cell matches it, so no arrow is drawn anywhere.
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
    natural.map_or_else(|| (clicked.to_owned(), SortDir::Asc), |f| (f.to_owned(), SortDir::Asc))
}

/// Spawn a fire-and-forget task that persists `view_id`'s sort field +
/// direction into `views.json`'s `view_sort`. A write failure is logged, not
/// surfaced — the in-memory re-sort already applied, so the only loss is
/// across a restart. Shared by every sortable view's `on_request_sort`.
pub(super) fn persist_view_sort(
    state: &AppState,
    view_id: &'static str,
    field: String,
    dir: SortDir,
) {
    let s = state.clone();
    let sort = ViewSort { field, dir };
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

/// Shadow of `views.json`'s `last_nav_index` plus the lock its writes serialize on.
///
/// `Nav.persist-selected-index` can fire twice in one tick — `nav_history::replay`
/// closes the departing detail first, and a close restores a cross-section origin —
/// and two `spawn_blocking` writes have no ordering between them, so the origin can
/// land last. `writer` supplies that ordering; `latest` lets a task holding it drop a
/// write a newer index has superseded. **The load has to sit under `writer`**, or both
/// tasks pass the check and race to land.
///
/// Two fields rather than a `Mutex<i32>` so the UI thread publishes with a store
/// instead of blocking on a guard held across file I/O.
struct NavIndexPersist {
    /// Published on the UI thread before each spawn; read by writers under `writer`.
    latest: AtomicI32,
    /// Held by the disk writers for the length of the write.
    writer: parking_lot::Mutex<()>,
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
            // Negative → 0 (try_from fails); then cap at the volume ceiling.
            let vol = u32::try_from(level).unwrap_or(0).min(crate::player::state::MAX_VOLUME);
            spawn_logged_sync!(s, "set_volume", library::playback::player_set_volume(&s.playback_ctx(), vol));
        });
    }

    // set_playback_speed: apply to the live player AND persist (speed
    // survives restarts — mirrors repeat/shuffle/volume). The flyout only
    // ever sends valid preset values; downstream clamps anyway, so no
    // clamp is needed here. Two steps like the gapless callback in
    // `src/ui/settings/playback_settings.rs`: (a) fast synchronous runtime apply,
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
            // `persist_blocking`, not the macro beside it: this writes
            // `settings.json`, and the macro is the `views.json` shape. Its
            // label already read as this one's, which was the tell.
            s.persist_blocking("persist playback_speed", move |st| {
                library::settings::set_playback_speed(st, speed)
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
        let persist = Arc::new(NavIndexPersist {
            latest: AtomicI32::new(ui.global::<Nav>().get_selected_index()),
            writer: parking_lot::Mutex::new(()),
        });
        nav.on_persist_selected_index(move |idx| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::ui::nav_history::record_current(&s, &ui);
            }
            // Ahead of the spawn — a queued write has to be able to see it.
            persist.latest.store(idx, Ordering::Release);
            // Spelled out rather than through `spawn_blocking_logged!`, which takes a
            // string *literal*: the index is what makes this line worth reading, and a
            // failure that doesn't say which section it dropped says almost nothing.
            let s_disk = s.clone();
            let persist = Arc::clone(&persist);
            s.runtime.spawn_blocking(move || {
                // Lock first: dropping a superseded write is only sound inside the
                // section that performs the write.
                let _write = persist.writer.lock();
                if persist.latest.load(Ordering::Acquire) != idx {
                    return;
                }
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

#[cfg(test)]
#[path = "tests/play_row_tests.rs"]
mod tests;

// A second flat `#[path]` mod rather than nesting both under `tests` — the
// play-row helpers reach `super::` for the fns they exercise, and nesting moves
// that one level away from this file.
#[cfg(test)]
#[path = "tests/nav_persist_tests.rs"]
mod nav_persist_tests;
