use std::sync::Arc;

use crate::config::Paths;
use crate::database::queries;
use crate::entities::track::TrackSummary;
use crate::error::{AppError, AppResult};
use crate::player::actions::emit_and_execute;
use crate::player::state::{play_track_inner, restore_queue, restore_station, with_state_emit};
use crate::player::types::{PersistedPlayback, RepeatMode};
use crate::services::settings::{mutate_settings, mutate_settings_with};
use crate::state::AppState;

use super::import::{ImportFilesResult, import_files_with_summaries};
use super::playback::player_play_tracks;

/// Order a freshly-imported batch the way the rest of the app orders a list.
///
/// Files from outside — an OS drop's `text/uri-list`, a file manager's "Open
/// with" — arrive in visual selection order on KDE / GNOME, not alphabetical.
/// `natord` puts "Track 2" before "Track 10"; stable, so equal titles keep their
/// import order.
fn sort_for_queue(summaries: &mut [Arc<TrackSummary>]) {
    summaries.sort_by(|a, b| natord::compare(&a.title, &b.title));
}

pub async fn queue_import_files(
    state: &AppState,
    file_paths: Vec<String>,
) -> Result<ImportFilesResult, AppError> {
    let mut result = import_files_with_summaries(state, &file_paths).await?;

    if !result.summaries.is_empty() {
        // Moved rather than cloned: the drag-drop caller discards the success
        // result, so an emptied `summaries` on it costs nothing.
        let mut summaries = std::mem::take(&mut result.summaries);
        sort_for_queue(&mut summaries);
        with_state_emit(&state.player_state, &state.sinks, |s| {
            s.queue.add_tracks(summaries);
        });
    }

    Ok(result)
}

/// The file-association entry point: import `file_paths`, make them the queue,
/// play from the top.
///
/// Replaces rather than appends — the queue you were listening to is not the
/// thing you just double-clicked. [`queue_import_files`] is the appending
/// sibling and the two differ in nothing else.
///
/// Ids come off the sorted summaries, not `ImportFilesResult::track_ids`, which
/// arrive partly out of a `HashMap` and would pick the first track by hash order.
pub async fn open_files(state: &AppState, file_paths: Vec<String>) -> AppResult<()> {
    let mut result = import_files_with_summaries(state, &file_paths).await?;

    if result.summaries.is_empty() {
        return Err(AppError::Queue(format!(
            "none of the {} opened file(s) could be read",
            file_paths.len()
        )));
    }

    sort_for_queue(&mut result.summaries);
    let track_ids: Vec<i64> = result.summaries.iter().map(|summary| summary.id).collect();

    log::debug!("queue: open {} track(s) from the command line", track_ids.len());
    player_play_tracks(&state.playback_ctx(), track_ids, Some(0)).await?;

    // An opened file is usually new to the library, so every view re-fetches —
    // the same bump the drag-and-drop import does.
    state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));
    Ok(())
}

pub async fn queue_add_tracks(state: &AppState, track_ids: Vec<i64>) -> Result<(), AppError> {
    let summaries: Vec<Arc<TrackSummary>> =
        queries::track::get_track_summaries_by_ids(&state.db, &track_ids)
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();

    log::debug!("queue: append {} track(s)", summaries.len());
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.add_tracks(summaries);
    });
    Ok(())
}

/// The context menu's "Play Next": insert after `current_index` in input order,
/// so the queue reads `[current, id_0, …, id_n, …]`. One DB round-trip and one
/// `with_state_emit` for the batch.
pub async fn queue_play_next_many(state: &AppState, track_ids: Vec<i64>) -> Result<(), AppError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let summaries: Vec<Arc<TrackSummary>> =
        queries::track::get_track_summaries_by_ids(&state.db, &track_ids)
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();

    log::debug!("queue: play next, {} track(s)", summaries.len());
    with_state_emit(&state.player_state, &state.sinks, |s| {
        // `insert_next` always lands at `current_index + 1`, so walking the
        // input backwards is what produces forward order.
        for summary in summaries.into_iter().rev() {
            s.queue.insert_next(summary);
        }
    });
    Ok(())
}

pub fn queue_remove(state: &AppState, index: usize) -> Result<(), AppError> {
    log::debug!("queue: remove index {index}");
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.remove_at(index);
    });
    Ok(())
}

pub fn queue_move(state: &AppState, from: usize, to: usize) -> Result<(), AppError> {
    // Once per drop, not per drag frame — Slint only emits on the drop gesture.
    log::debug!("queue: move {from} → {to}");
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.move_track(from, to);
    });
    Ok(())
}

pub fn queue_remove_batch(state: &AppState, indices: &[usize]) -> Result<(), AppError> {
    log::debug!("queue: remove {} selected", indices.len());
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.remove_batch(indices);
    });
    Ok(())
}

pub fn queue_clear(state: &AppState) -> Result<(), AppError> {
    // The one queue edit that can leave no other trace: with nothing playing it
    // emits no `PlayerAction`.
    log::debug!("queue: clear");
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.clear();
    });
    Ok(())
}

pub fn queue_skip_to_index(state: &AppState, index: usize) -> Result<(), AppError> {
    // The `play` line this produces names the file; only this names why.
    log::debug!("queue: skip to index {index}");
    emit_and_execute(&*state.engine, &state.db, &state.player_state, &state.sinks, |s| {
        let track = s.queue.skip_to_index(index).cloned();
        match track {
            Some(t) => play_track_inner(s, t, None),
            None => vec![],
        }
    });
    Ok(())
}

/// Drive shuffle to `enabled`, doing nothing when it is already there.
///
/// Comparison and flip share one `with_state_emit` closure, so a caller meaning
/// "on" can't be raced into "off" by the transport button landing between a read
/// and a write — the whole reason to reach for this over
/// read-then-`queue_toggle_shuffle`. The toggle is the transport's own path,
/// where flipping whatever is current *is* the intent.
pub fn queue_set_shuffle(state: &AppState, enabled: bool) -> Result<(), AppError> {
    let new_shuffle = with_state_emit(&state.player_state, &state.sinks, |s| {
        if s.queue.shuffle_enabled == enabled {
            return s.queue.shuffle_enabled;
        }

        if enabled {
            shuffle_inline(s);
        } else {
            s.queue.unshuffle();
        }
        s.queue.shuffle_enabled
    });
    // The resulting state rather than the request: this is the idempotent form,
    // so a Shuffle pill pressed twice asks for `true` twice and only moves once.
    log::debug!("queue: shuffle {new_shuffle}");
    persist_shuffle(state, new_shuffle);
    Ok(())
}

pub fn queue_toggle_shuffle(state: &AppState) -> Result<(), AppError> {
    let new_shuffle = with_state_emit(&state.player_state, &state.sinks, |s| {
        if s.queue.shuffle_enabled {
            s.queue.unshuffle();
        } else {
            shuffle_inline(s);
        }
        s.queue.shuffle_enabled
    });
    log::debug!("queue: shuffle {new_shuffle}");
    persist_shuffle(state, new_shuffle);
    Ok(())
}

pub fn queue_set_repeat(state: &AppState, mode: RepeatMode) -> Result<(), AppError> {
    log::debug!("queue: repeat → {mode:?}");
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.set_repeat_mode(mode);
    });
    persist_repeat(state, mode);
    Ok(())
}

pub fn queue_cycle_repeat(state: &AppState) -> Result<(), AppError> {
    let new_mode = with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.cycle_repeat_mode();
        s.queue.repeat_mode
    });
    log::debug!("queue: repeat → {new_mode:?}");
    persist_repeat(state, new_mode);
    Ok(())
}

/// Fire-and-forget settings.json write for shuffle, on the blocking pool so the
/// `wire_sync!` spawn returns immediately. A failed write only logs: the
/// in-memory state already changed and persistence is best-effort.
fn persist_shuffle(state: &AppState, enabled: bool) {
    let paths = state.paths.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = mutate_settings(&paths, |s| {
            s.queue.shuffle_enabled = enabled;
        }) {
            log::warn!("persist shuffle_enabled: {e}");
        }
    });
}

fn persist_repeat(state: &AppState, mode: RepeatMode) {
    let paths = state.paths.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = mutate_settings(&paths, |s| {
            s.queue.repeat_mode = mode;
        }) {
            log::warn!("persist repeat_mode: {e}");
        }
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

/// Startup restore: what the last session was playing back into `PlayerState`, once, from
/// `boot::tasks`.
///
/// The queue and the station over it come back together, in that order — seating the station
/// clears the `current_track` the queue restore just set, and the queue underneath is what a stop
/// hands back (D9).
///
/// `repeat_mode` and `shuffle_enabled` come from `settings.json`, not
/// `queue.json`. A restored non-empty queue forces shuffle off (and syncs
/// `settings.json` to match) because `original_order` isn't persisted — left on,
/// "unshuffle" would be a no-op against an unknown original sequence.
pub async fn restore_persisted_playback(state: &AppState) -> AppResult<()> {
    let persisted = load_persisted_playback(&state.paths);
    let (summaries, persisted) = match persisted {
        Some(p) => {
            let summaries: Vec<Arc<TrackSummary>> =
                queries::track::get_track_summaries_by_ids(&state.db, &p.queue.track_ids)
                    .await?
                    .into_iter()
                    .map(Arc::new)
                    .collect();
            (summaries, Some(p))
        }
        None => (Vec::new(), None),
    };

    let station = match persisted.as_ref().and_then(|p| p.station_id) {
        Some(id) => super::radio::station_to_restore(state, id).await,
        None => None,
    };

    // One FS round-trip, all under the same `MUTATE_LOCK` so no writer
    // interleaves between the read and the conditional rewrite.
    let restored_non_empty = !summaries.is_empty();
    let paths = state.paths.clone();
    let (settings_repeat, settings_shuffle) =
        tokio::task::spawn_blocking(move || -> AppResult<_> {
            mutate_settings_with(&paths, |s| {
                let prior_repeat = s.queue.repeat_mode;
                let prior_shuffle = s.queue.shuffle_enabled;
                if restored_non_empty && prior_shuffle {
                    s.queue.shuffle_enabled = false;
                }
                (prior_repeat, prior_shuffle)
            })
        })
        .await
        .map_err(|e| AppError::Settings(format!("restore_persisted_playback join: {e}")))??;

    // `emit_and_execute` rather than a bare emit because a seated station may owe the deck a speed
    // reset: `settings.json` hydrates the speed ahead of this and a station is pinned at 1.0.
    emit_and_execute(&*state.engine, &state.db, &state.player_state, &state.sinks, |s| {
        if let Some(p) = persisted.as_ref() {
            restore_queue(s, summaries, &p.queue);
        }
        s.queue.repeat_mode = settings_repeat;
        // Force shuffle off when a non-empty queue is restored — see doc.
        s.queue.shuffle_enabled = if restored_non_empty {
            false
        } else {
            settings_shuffle
        };
        match station {
            Some(station) => restore_station(s, station),
            None => vec![],
        }
    });

    Ok(())
}

/// Shuffle the queue's `play_order` in place using a thread-local RNG,
/// pinning the currently-playing track to position 0 so playback stays
/// continuous. Allocation-free — no intermediate index Vec.
fn shuffle_inline(state: &mut crate::player::state::PlayerState) {
    let mut rng = rand::rng();
    state.queue.shuffle_play_order_in_place(&mut rng, /* anchor_to_current */ true);
}

#[cfg(test)]
#[path = "tests/queue_tests.rs"]
mod tests;
