//! Pull-side glue: every `Player.on_*` callback declared in the Slint UI is wired here to
//! a `library::*` function, on the event-loop thread, dispatching the work onto tokio.
//!
//! What is left here is the cross-cutting set, answering to no single view. `wire_all`
//! wires the `Player` global itself plus the `Nav` persist callback; each view slice owns
//! its wiring under `ui/<view>/callbacks/`.

// `pub(in crate::ui)` rather than private: each view slice owns its own `callbacks/`
// submodule, so the wiring reaching these isn't a descendant of this module. Still sealed
// inside `ui::` — nothing in `library/`, `player/` or `boot/` can name either.
pub(in crate::ui) mod macros;

pub(in crate::ui) mod cross_tab_nav;
pub(in crate::ui) mod index_persist;
mod library_settings;
mod now_playing;
mod tags;
mod updater;

use std::sync::Arc;

use rand::RngExt;
use slint::{ComponentHandle, Model, ModelRc};

use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::services::view_state::ViewStateData;
use crate::state::AppState;
use crate::{AppWindow, Nav, Player};

use index_persist::IndexPersist;
use macros::{spawn_logged_sync, wire_pb, wire_sync, wire_sync_pb};

pub use cross_tab_nav::wire_cross_tab_nav;
pub use library_settings::wire_library_settings;
pub use now_playing::{wire_now_playing_favorite, wire_now_playing_rating};
pub use tags::wire_tags;
pub use updater::wire as wire_updater;

/// A Slint `[int]` callback param as `Vec<i64>`, for every track-row context-menu callback
/// — single-row mode emits a 1-element array, multi-select the whole view selection.
pub(super) fn collect_track_ids(ids: &ModelRc<i32>) -> Vec<i64> {
    ids.iter().map(i64::from).collect()
}

/// The variant that drops `id == 0` — Browse's disk-only rows, which aren't in the
/// library and can't be queued or favorited. `browse/selection.rs` already filters them
/// upstream; this guards the single-row context-menu path.
pub(super) fn collect_nonzero_track_ids(ids: &ModelRc<i32>) -> Vec<i64> {
    ids.iter().filter(|&id| id != 0).map(i64::from).collect()
}

/// Resolve the queue slot a `play-row` activation should start on. The row index a view
/// hands over indexes its *displayed* rows, the same space as `ids` everywhere but Browse,
/// which also shows disk-only files that `current_in_library_ids` drops — so fall back to
/// a lookup by id, and to the head of the queue if the track isn't in the list at all.
pub(super) fn play_row_start(ids: &[i64], track_id: i64, idx: i32) -> Option<usize> {
    if let Ok(i) = usize::try_from(idx)
        && ids.get(i) == Some(&track_id)
    {
        return Some(i);
    }
    ids.iter().position(|&id| id == track_id)
}

/// Track ids of a live `VecModel<TrackListRow>`, in display order. Only Search needs this:
/// every other view caches its displayed rows on a `*Ui` handle, where Search's visible
/// set is a sorted, truncated projection of `last_results` assembled at render time, so
/// the model is the only place its display order exists. LIMIT-bounded, so the UI-thread
/// walk is cheap.
pub(super) fn model_track_ids(rows: &ModelRc<crate::TrackListRow>) -> Vec<i64> {
    rows.iter().map(|r| i64::from(r.id)).collect()
}

/// Replace the queue with `ids`, open on a random one, then turn shuffle on — the header
/// Shuffle pill on the six views that carry one.
///
/// The two halves have to be sequenced, shuffle re-anchoring around whatever is playing.
/// `queue_set_shuffle` rather than a read-then-toggle pair: the pill means "on", and a
/// toggle racing the transport's own button would turn it off. The start slot is random
/// because the shuffle anchors the current track at the front, so starting at the head
/// opens every press on the same song.
pub(super) fn spawn_play_then_shuffle(state: &AppState, tag: &'static str, ids: Vec<i64>) {
    // Not just an early exit: `random_range` panics on an empty range, and a filter
    // matching nothing leaves a live pill over an empty list, the header gating on the
    // view's *unfiltered* count.
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

/// Resolve a sort-pill (or column-header) click against the sort in force: the field
/// already sorted on flips direction, a different one starts ascending.
///
/// `cur_dir` is read the way [`SortDir::from_token`] reads it — only `"desc"` is
/// descending — rather than testing for `"asc"`. The two differ only on a token neither
/// side can currently produce, but disagreeing leaves that pill unable to reach descending
/// at all.
pub(super) fn next_sort(cur_field: &str, cur_dir: &str, clicked: &str) -> (String, SortDir) {
    next_sort_with_natural(cur_field, cur_dir, clicked, None)
}

/// [`next_sort`] for a view with an order of its own to fall back to: ascending,
/// descending, then `natural`.
///
/// A *synthetic* order — one no column header can ask for — is otherwise unreachable the
/// moment anything else is clicked, and the sort persists across restarts. Playlist Detail
/// is the only caller: `"position"` is what drag-to-reorder is gated on, so one click on
/// Title used to retire reordering for good. Nothing paints the third state, no header
/// cell matching the natural field.
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

/// Persist `view_id`'s sort into `views.json`, fire-and-forget: the in-memory re-sort
/// already applied, so the only loss is across a restart. Shared by every
/// `on_request_sort`.
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

/// `view_id`'s persisted sort, or `None` on a fresh install, where the caller keeps its
/// Slint-declared default. Each view's `wire_*` uses it to seed the sort header at startup,
/// and `boot::ui_setup` for the Tracks fetch that has to agree with one.
///
/// Takes the state `boot` already read rather than reaching for `views.json` itself: this runs
/// once per view on the main thread ahead of `app.show()`, and the file has been parsed since
/// `main`, so a second read per view buys nothing but the chance to disagree with the first.
///
/// Borrowed rather than handed back as owned strings: most callers only push the field into a
/// `SharedString`, and the ones that keep it clone where they keep it.
pub fn persisted_sort<'a>(
    view_state: Option<&'a ViewStateData>,
    view_id: &str,
) -> Option<&'a ViewSort> {
    view_state?.view_sort.get(view_id)
}

/// Wire every Slint `Player.*` callback to its `library::*` counterpart.
/// Call once after constructing `AppWindow`.
pub fn wire_all(ui: &AppWindow, state: &AppState) {
    let player = ui.global::<Player>();
    let ui_weak = ui.as_weak();

    wire_sync_pb!(
        player,
        on_play_pause,
        state,
        "play_pause",
        library::playback::player_toggle_play_pause
    );
    wire_sync_pb!(player, on_next, state, "next", library::playback::player_next);
    wire_sync_pb!(player, on_previous, state, "previous", library::playback::player_previous);
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

    // seek: hold the slider at the requested position until the backend reports a
    // matching update (`Player.seek_pending_ms`).
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
            let vol =
                u32::try_from(level).unwrap_or(0).min(crate::player::engine::state::MAX_VOLUME);
            spawn_logged_sync!(
                s,
                "set_volume",
                library::playback::player_set_volume(&s.playback_ctx(), vol)
            );
        });
    }

    // set_playback_speed: apply to the live player *and* persist, speed surviving restarts
    // as repeat, shuffle and volume do. Two steps, as the gapless callback takes: a fast
    // synchronous apply, then a blocking-pool write.
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
            // `persist_blocking`, not the macro beside it: this writes `settings.json`,
            // where the macro is the `views.json` shape.
            s.persist_blocking("persist playback_speed", move |st| {
                library::settings::set_playback_speed(st, speed)
            });
        });
    }

    // Player.toggle-favorite is wired in `wire_now_playing_favorite` (called
    // after every per-view wire fn) so it can fan the change into all three
    // surfaces that hold a per-row `is_favorite` (Tracks, Browse, AlbumDetail).

    // Nav.persist-selected-index: fired after every sidebar click. Persists
    // `last_nav_index` on the blocking pool and records a history entry so Mouse-4/5 can
    // walk back through tab switches. `record_current` reads on the UI thread ahead of any
    // disk hop, off the post-click index and detail id, so the entry is what the user is
    // about to see.
    let nav = ui.global::<Nav>();
    {
        let s = state.clone();
        let ui_weak = ui_weak.clone();
        // `Nav.persist-selected-index` can fire twice in one tick — `nav_history::replay`
        // closes the departing detail first, and a close restores a cross-section origin.
        let persist = Arc::new(IndexPersist::new(nav.get_selected_index()));
        nav.on_persist_selected_index(move |idx| {
            if let Some(ui) = ui_weak.upgrade() {
                crate::ui::nav_history::record_current(&ui);
            }
            persist.publish(idx);
            // Spelled out rather than through `spawn_blocking_logged!`, which takes a
            // string *literal*: a failure has to name the section it dropped.
            let s_disk = s.clone();
            let persist = Arc::clone(&persist);
            s.runtime.spawn_blocking(move || {
                persist.write_if_current(idx, || {
                    if let Err(e) = library::settings::set_last_nav_index(&s_disk, idx) {
                        log::warn!("nav: set_last_nav_index({idx}): {e}");
                    }
                });
            });
        });
    }

    // Nav.reveal-in-folder: "Open Containing Folder", in every track-row context menu.
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

// A second flat `#[path]` mod rather than nesting both under `tests`: the play-row
// helpers reach `super::` for the fns they exercise, and nesting moves that one level
// away from this file.
#[cfg(test)]
#[path = "tests/index_persist_tests.rs"]
mod index_persist_tests;
