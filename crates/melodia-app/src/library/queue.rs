use std::sync::Arc;

use crate::services::settings::{mutate_settings, mutate_settings_with};
use crate::state::{AppState, PlaybackContext};
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::config::Paths;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::{AppError, AppResult};
use melodia_engine::player::engine::actions::emit_and_execute;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::queue::{
    ClearedQueue, QueueCut, QueueEntry, QueueState, current_index_from_i32,
};
use melodia_engine::player::engine::state::{
    PlayerAction, PlayerState, PlayerStateHandle, ReplacedQueue, lock_state, restore_queue,
    restore_station, with_state_emit,
};
use melodia_engine::player::engine::types::{PersistedPlayback, RadioNowPlaying, RepeatMode};
use melodia_store::database::{DbPool, queries};

use super::import::{ImportFilesResult, import_and_summarize};
use super::keyset::existing_in_order;
use super::playback::player_play_tracks;

/// Files from outside, a drop or "Open with", arrive in visual selection order. Stable, so equal
/// titles keep their import order.
fn sort_for_queue(summaries: &mut [Arc<TrackSummary>]) {
    summaries.sort_by(|a, b| natord::compare(&a.title, &b.title));
}

pub async fn queue_import_files(
    state: &AppState,
    file_paths: Vec<String>,
) -> Result<ImportFilesResult, AppError> {
    append_imported(&state.playback_ctx(), &state.cover_cache, &file_paths).await
}

/// Import `file_paths` and append them to whatever is already queued.
async fn append_imported(
    ctx: &PlaybackContext,
    cover_cache: &CoverCache,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    let mut result =
        import_and_summarize(&ctx.db, &ctx.paths.artwork_dir, cover_cache, file_paths).await?;

    if !result.summaries.is_empty() {
        // Moved rather than cloned: the drag-drop caller discards the success result.
        let mut summaries = std::mem::take(&mut result.summaries);
        sort_for_queue(&mut summaries);
        let ids: Vec<i64> = summaries.iter().map(|summary| summary.id).collect();
        with_state_emit(&ctx.player_state, &ctx.sinks, |s| {
            s.queue.add_tracks(&ids);
        });
    }

    Ok(result)
}

/// The file-association entry point: import `file_paths`, make them the queue, play from the top.
/// Replaces rather than appends, since the queue you were listening to is not what you just
/// double-clicked.
pub async fn open_files(state: &AppState, file_paths: Vec<String>) -> AppResult<()> {
    open_as_queue(&state.playback_ctx(), &state.cover_cache, &file_paths).await?;

    // An opened file is usually new to the library.
    state.library_changed.bump();
    Ok(())
}

/// Ids come off the sorted summaries, not `ImportFilesResult::track_ids`, which arrive partly in
/// `HashMap` order.
async fn open_as_queue(
    ctx: &PlaybackContext,
    cover_cache: &CoverCache,
    file_paths: &[String],
) -> AppResult<()> {
    let mut result =
        import_and_summarize(&ctx.db, &ctx.paths.artwork_dir, cover_cache, file_paths).await?;

    if result.summaries.is_empty() {
        return Err(AppError::queue_msg(format!(
            "none of the {} opened file(s) could be read",
            file_paths.len()
        )));
    }

    sort_for_queue(&mut result.summaries);
    let track_ids: Vec<i64> = result.summaries.iter().map(|summary| summary.id).collect();

    log::debug!("queue: open {} track(s) from the command line", track_ids.len());
    player_play_tracks(ctx, track_ids, Some(0)).await
}

pub async fn queue_add_tracks(state: &AppState, track_ids: Vec<i64>) -> Result<(), AppError> {
    let ids = existing_in_order(&state.db, &track_ids).await?;

    log::debug!("queue: append {} track(s)", ids.len());
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.add_tracks(&ids);
    });
    Ok(())
}

/// A drop into the queue panel: splice `track_ids` in ahead of the row `before`, or at the end,
/// in the order they came. The drop names a row rather than a slot because the resolve awaits the
/// database, and the queue can move under a slot meanwhile.
pub async fn queue_insert_before(
    state: &AppState,
    before: Option<QueueEntry>,
    track_ids: Vec<i64>,
) -> Result<(), AppError> {
    let ids = existing_in_order(&state.db, &track_ids).await?;

    log::debug!("queue: insert {} track(s) before {before:?}", ids.len());
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.insert_before(before, &ids);
    });
    Ok(())
}

/// "Play Next": inserted after the current track in input order.
pub async fn queue_play_next_many(state: &AppState, track_ids: Vec<i64>) -> Result<(), AppError> {
    play_next_many(&state.db, &state.player_state, &state.sinks, &track_ids).await
}

/// Narrowed to what it reaches, so the ordering can be driven off a bare pool.
async fn play_next_many(
    db: &DbPool,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    track_ids: &[i64],
) -> Result<(), AppError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let ids = existing_in_order(db, track_ids).await?;

    log::debug!("queue: play next, {} track(s)", ids.len());
    with_state_emit(player_state, sinks, |s| {
        s.queue.insert_next(&ids);
    });
    Ok(())
}

/// Takes the rows `entries` name, one row or every pick, and hands back what puts them back.
pub fn queue_remove(state: &AppState, entries: &[QueueEntry]) -> Option<QueueCut> {
    log::debug!("queue: remove {} row(s)", entries.len());
    with_state_emit(&state.player_state, &state.sinks, |s| s.cut_queue(entries))
}

/// Puts back a run of removals, playback untouched: the removals never touched it.
pub fn queue_restore_cuts(state: &AppState, cuts: Vec<QueueCut>) {
    log::debug!("queue: undo {} removal(s)", cuts.len());
    with_state_emit(&state.player_state, &state.sinks, |s| s.restore_queue_cuts(cuts));
}

/// Moves the play-order slots `rows` together to `to`, counted once they have left: one dragged
/// row, or every row the panel holds picked.
pub fn queue_move(state: &AppState, rows: &[usize], to: usize) -> Result<(), AppError> {
    // Once per drop: Slint emits on the drop gesture alone.
    log::debug!("queue: move {} row(s) → {to}", rows.len());
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.move_rows(rows, to);
    });
    Ok(())
}

/// Empties the queue, the playing track playing on, and hands back what it held.
pub fn queue_clear(state: &AppState) -> Option<ClearedQueue> {
    // The one queue edit that can leave no other trace: with nothing playing it emits no action.
    log::debug!("queue: clear");
    with_state_emit(&state.player_state, &state.sinks, |s| s.queue.take_cleared())
}

/// Puts back a run of clears, ahead of anything queued since.
pub fn queue_restore_cleared(state: &AppState, cleared: Vec<ClearedQueue>) {
    log::debug!("queue: undo {} clear(s)", cleared.len());
    with_state_emit(&state.player_state, &state.sinks, |s| s.restore_cleared_queue(cleared));
}

/// Puts back the queue a play replaced, and what was playing over it.
pub fn queue_restore_replaced(state: &AppState, replaced: ReplacedQueue) {
    log::debug!("queue: undo a replace");
    emit_and_execute(&*state.engine, &state.player_state, &state.sinks, |s| {
        s.restore_replaced_queue(replaced, &mut rand::rng())
    });
}

/// Play the queue's track at `index`. Its record is fetched first unless the window holds it, a
/// double-click being able to land anywhere in a queue the size of the library.
///
/// The row is carried across the fetch as its entry, the queue being free to move under an index
/// meanwhile, and one that has left by then plays nothing.
pub async fn queue_skip_to_index(state: &AppState, index: usize) -> Result<(), AppError> {
    // The `play` line this produces names the file; only this names why.
    log::debug!("queue: skip to index {index}");
    let (entry, unheld) = {
        let s = lock_state(&state.player_state);
        let Some(entry) = s.queue.entry_at(index) else {
            return Ok(());
        };
        (entry, s.queue.id_at(index).filter(|&id| s.record(id).is_none()))
    };
    let fetched = match unheld {
        Some(id) => queries::track::get_track_summaries_by_ids(&state.db, &[id]).await?,
        None => Vec::new(),
    };
    emit_and_execute(&*state.engine, &state.player_state, &state.sinks, |s| {
        let Some(id) = s.queue.skip_to_entry(entry) else {
            return vec![];
        };
        s.window.seat(&s.queue.window_ids(), fetched.into_iter().map(Arc::new));
        s.start_queued(id, false, 0)
    });
    Ok(())
}

/// Drive shuffle to `enabled`. The comparison and the flip share one emit, so the transport button
/// landing between a read and a write can't turn "on" into "off", which a read then
/// [`queue_toggle_shuffle`] would allow.
pub fn queue_set_shuffle(state: &AppState, enabled: bool) -> Result<(), AppError> {
    let new_shuffle = set_shuffle(&state.player_state, &state.sinks, enabled);
    log::debug!("queue: shuffle {new_shuffle}");
    persist_shuffle(state, new_shuffle);
    Ok(())
}

pub fn queue_toggle_shuffle(state: &AppState) -> Result<(), AppError> {
    let new_shuffle = toggle_shuffle(&state.player_state, &state.sinks);
    log::debug!("queue: shuffle {new_shuffle}");
    persist_shuffle(state, new_shuffle);
    Ok(())
}

pub fn queue_cycle_repeat(state: &AppState) -> Result<(), AppError> {
    let new_mode = cycle_repeat(&state.player_state, &state.sinks);
    log::debug!("queue: repeat → {new_mode:?}");
    persist_repeat(state, new_mode);
    Ok(())
}

/// Drive repeat to `mode`, for a caller naming the mode rather than stepping through them.
pub fn queue_set_repeat(state: &AppState, mode: RepeatMode) -> Result<(), AppError> {
    let new_mode = set_repeat(&state.player_state, &state.sinks, mode);
    log::debug!("queue: repeat → {new_mode:?}");
    persist_repeat(state, new_mode);
    Ok(())
}

/// Answers with the shuffle state it settled in, which is *not* the request: an empty queue
/// refuses to shuffle, so a caller persisting `enabled` writes a shuffle the next launch
/// restores over a queue that was never reordered.
fn set_shuffle(player_state: &PlayerStateHandle, sinks: &PlayerSinks, enabled: bool) -> bool {
    with_state_emit(player_state, sinks, |s| {
        if s.queue.shuffle_enabled == enabled {
            return s.queue.shuffle_enabled;
        }

        if enabled {
            shuffle_inline(s);
        } else {
            s.queue.unshuffle();
        }
        s.queue.shuffle_enabled
    })
}

/// Flip shuffle, answering with what it became.
fn toggle_shuffle(player_state: &PlayerStateHandle, sinks: &PlayerSinks) -> bool {
    with_state_emit(player_state, sinks, |s| {
        if s.queue.shuffle_enabled {
            s.queue.unshuffle();
        } else {
            shuffle_inline(s);
        }
        s.queue.shuffle_enabled
    })
}

/// Advance the repeat mode, answering with the one it landed on rather than the one it left.
fn cycle_repeat(player_state: &PlayerStateHandle, sinks: &PlayerSinks) -> RepeatMode {
    with_state_emit(player_state, sinks, |s| {
        s.queue.cycle_repeat_mode();
        s.queue.repeat_mode
    })
}

/// Set the repeat mode, answering with the one it landed on.
fn set_repeat(
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    mode: RepeatMode,
) -> RepeatMode {
    with_state_emit(player_state, sinks, |s| {
        s.queue.set_repeat_mode(mode);
        s.queue.repeat_mode
    })
}

/// Best-effort: the state has already changed, so a failed write only logs.
fn persist_shuffle(state: &AppState, enabled: bool) {
    state.persist_blocking("persist shuffle_enabled", move |paths| {
        mutate_settings(paths, |s| s.queue.shuffle_enabled = enabled)
    });
}

fn persist_repeat(state: &AppState, mode: RepeatMode) {
    state.persist_blocking("persist repeat_mode", move |paths| {
        mutate_settings(paths, |s| s.queue.repeat_mode = mode)
    });
}

/// Read `queue.json`. Best-effort: a missing or unparseable file restores nothing.
fn load_persisted_playback(paths: &Paths) -> Option<PersistedPlayback> {
    if !paths.queue_path.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&paths.queue_path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Startup restore of what the last session was playing. Gathered, then applied: [`plan_restore`]
/// needs a pool, the apply an audio device. The station resolves here, being the one part behind
/// the radio facade's off switch.
pub async fn restore_persisted_playback(state: &AppState) -> AppResult<()> {
    let persisted = load_persisted_playback(&state.paths);
    let station = match persisted.as_ref().and_then(|p| p.station_id) {
        Some(id) => super::radio::station_to_restore(state, id).await,
        None => None,
    };
    let plan = plan_restore(&state.db, &state.paths, persisted, station).await?;

    // `emit_and_execute` rather than a bare emit because a seated station may owe the deck a speed
    // reset: `settings.json` hydrates the speed ahead of this and a station is pinned at 1.0.
    emit_and_execute(&*state.engine, &state.player_state, &state.sinks, |s| apply_restore(s, plan));

    Ok(())
}

/// Everything a restart puts back, resolved before anything touches `PlayerState`.
struct RestorePlan {
    /// The saved queue less the rows gone since, its shuffle and repeat settled.
    queue: QueueState,
    /// The records of `queue`'s window, the current track's among them.
    records: Vec<Arc<TrackSummary>>,
    station: Option<Arc<RadioNowPlaying>>,
}

/// Resolve `queue.json`'s ids against the library and settle shuffle and repeat against
/// `settings.json`.
///
/// A row gone since the save drops out and the current slot moves to the next survivor, as a prune
/// mid-session does. A restored non-empty queue forces shuffle off, `original_order` not being
/// persisted, and the file is rewritten in the same closure so state and disk can't disagree.
/// Settled before the window is read, which wraps where repeat does.
async fn plan_restore(
    db: &DbPool,
    paths: &Arc<Paths>,
    persisted: Option<PersistedPlayback>,
    station: Option<Arc<RadioNowPlaying>>,
) -> AppResult<RestorePlan> {
    let (ids, current_index) = persisted.map_or((Vec::new(), None), |p| {
        (p.queue.track_ids, current_index_from_i32(p.queue.current_index))
    });
    let gone = queries::track::missing_ids(db, &ids).await?;
    let mut queue = QueueState::restored(ids, current_index);
    queue.prune_missing(&gone);

    let restored_non_empty = !queue.is_empty();
    let paths = Arc::clone(paths);
    let (repeat_mode, shuffle_enabled) = tokio::task::spawn_blocking(move || -> AppResult<_> {
        mutate_settings_with(&paths, |s| {
            if restored_non_empty {
                s.queue.shuffle_enabled = false;
            }
            (s.queue.repeat_mode, s.queue.shuffle_enabled)
        })
    })
    .await
    .map_err(|e| AppError::io("restore_persisted_playback join", e))??;
    queue.repeat_mode = repeat_mode;
    queue.shuffle_enabled = shuffle_enabled;

    let records = queries::track::get_track_summaries_by_ids(db, &queue.window_ids())
        .await?
        .into_iter()
        .map(Arc::new)
        .collect();
    Ok(RestorePlan { queue, records, station })
}

/// The queue first and the station over it: seating the station clears the `current_track` the
/// queue restore set, and the queue underneath is what a stop hands back.
fn apply_restore(state: &mut PlayerState, plan: RestorePlan) -> Vec<PlayerAction> {
    let RestorePlan { queue, records, station } = plan;

    restore_queue(state, queue, records);

    match station {
        Some(station) => restore_station(state, station),
        None => vec![],
    }
}

/// Pins the playing track to position 0, so playback stays continuous.
fn shuffle_inline(state: &mut PlayerState) {
    let mut rng = rand::rng();
    state.queue.shuffle_play_order_in_place(&mut rng, /* anchor_to_current */ true);
}

#[cfg(test)]
#[path = "tests/queue_tests.rs"]
mod tests;
