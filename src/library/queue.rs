use std::sync::Arc;

use crate::database::queries;
use crate::entities::track::TrackSummary;
use crate::error::{AppError, AppResult};
use crate::player::actions::execute_actions;
use crate::player::rodio_backend::load_queue_from_disk_sync;
use crate::player::state::{play_track_inner, restore_queue, with_state_emit};
use crate::player::types::RepeatMode;
use crate::services::settings::{mutate_settings, mutate_settings_with};
use crate::state::AppState;

use super::import::{ImportFilesResult, import_files_with_summaries};

pub async fn queue_import_files(
    state: &AppState,
    file_paths: Vec<String>,
) -> Result<ImportFilesResult, AppError> {
    let mut result = import_files_with_summaries(state, &file_paths).await?;

    if !result.summaries.is_empty() {
        // Move summaries into the queue without cloning — the caller
        // (drag-drop handler in `window_chrome`) discards the success
        // result, so leaving `summaries` empty on the returned value is
        // harmless.
        let mut summaries = std::mem::take(&mut result.summaries);
        // Multi-file OS drops land in whatever order the file manager
        // delivered them in the `text/uri-list` payload (usually visual
        // selection order on KDE / GNOME, not alphabetical). Sort by
        // title with `natord` so the queue order matches the way the
        // rest of the app sorts library views — "Track 2" lands before
        // "Track 10". Stable so already-identical titles keep their
        // import order.
        summaries.sort_by(|a, b| natord::compare(&a.title, &b.title));
        with_state_emit(&state.player_state, &state.sinks, |s| {
            s.queue.add_tracks(summaries);
        });
    }

    Ok(result)
}

pub async fn queue_add_tracks(state: &AppState, track_ids: Vec<i64>) -> Result<(), AppError> {
    let summaries: Vec<Arc<TrackSummary>> =
        queries::track::get_track_summaries_by_ids(&state.db, &track_ids)
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();

    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.add_tracks(summaries);
    });
    Ok(())
}

/// Play a single track immediately while preserving the queue. If the
/// track is already queued, skip-to its position; otherwise append it
/// at the tail and skip-to the newly-added slot. Either way the queue
/// keeps its existing contents (no flood) and the clicked track plays
/// straight away — matches the universal "double-click plays this" UX
/// without `player_play_tracks`'s wipe-and-reload.
pub async fn queue_append_unique(state: &AppState, track_id: i64) -> Result<(), AppError> {
    let summary = queries::track::get_track_summary_by_id(&state.db, track_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("track {track_id}")))?;
    let summary = Arc::new(summary);

    let actions = with_state_emit(&state.player_state, &state.sinks, |s| {
        let existing = s
            .queue
            .play_order
            .iter()
            .position(|&ti| s.queue.tracks.get(ti).is_some_and(|t| t.id == track_id));
        let target_idx = existing.unwrap_or_else(|| {
            s.queue.add_tracks(vec![summary]);
            s.queue.play_order.len() - 1
        });
        let track = s.queue.skip_to_index(target_idx).cloned();
        match track {
            Some(t) => play_track_inner(s, t, None),
            None => vec![],
        }
    });

    execute_actions(
        actions,
        &*state.rodio,
        &state.db,
        &state.player_state,
        &state.sinks,
    );
    Ok(())
}

pub async fn queue_play_next(state: &AppState, track_id: i64) -> Result<(), AppError> {
    let summary = queries::track::get_track_summary_by_id(&state.db, track_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("track {track_id}")))?;
    let summary = Arc::new(summary);

    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.insert_next(summary);
    });
    Ok(())
}

/// Batch form of [`queue_play_next`]. Inserts every track in `track_ids`
/// directly after `current_index`, preserving the input order so the
/// user-visible queue ends up as `[current, id_0, id_1, …, id_n, …]`.
/// One DB round-trip + one `with_state_emit` for the whole batch.
pub async fn queue_play_next_many(
    state: &AppState,
    track_ids: Vec<i64>,
) -> Result<(), AppError> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let summaries: Vec<Arc<TrackSummary>> =
        queries::track::get_track_summaries_by_ids(&state.db, &track_ids)
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();

    with_state_emit(&state.player_state, &state.sinks, |s| {
        // `insert_next` puts the argument at `current_index + 1`, so
        // walking the input in reverse produces forward order in the
        // queue: feeding c, b, a one-by-one ends up as
        // `[current, a, b, c, …]`.
        for summary in summaries.into_iter().rev() {
            s.queue.insert_next(summary);
        }
    });
    Ok(())
}

pub fn queue_remove(state: &AppState, index: usize) -> Result<(), AppError> {
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.remove_at(index);
    });
    Ok(())
}

pub fn queue_move(state: &AppState, from: usize, to: usize) -> Result<(), AppError> {
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.move_track(from, to);
    });
    Ok(())
}

pub fn queue_remove_batch(state: &AppState, indices: &[usize]) -> Result<(), AppError> {
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.remove_batch(indices);
    });
    Ok(())
}

pub fn queue_clear(state: &AppState) -> Result<(), AppError> {
    with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.clear();
    });
    Ok(())
}

pub fn queue_skip_to_index(state: &AppState, index: usize) -> Result<(), AppError> {
    let actions = with_state_emit(&state.player_state, &state.sinks, |s| {
        let track = s.queue.skip_to_index(index).cloned();
        match track {
            Some(t) => play_track_inner(s, t, None),
            None => vec![],
        }
    });

    execute_actions(
        actions,
        &*state.rodio,
        &state.db,
        &state.player_state,
        &state.sinks,
    );
    Ok(())
}

pub async fn queue_direct_play(state: &AppState, track_id: i64) -> Result<(), AppError> {
    let summary = queries::track::get_track_summary_by_id(&state.db, track_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("track {track_id}")))?;
    let summary = Arc::new(summary);

    let actions = with_state_emit(&state.player_state, &state.sinks, |s| {
        s.queue.set_direct_play(Arc::clone(&summary));
        play_track_inner(s, summary, None)
    });

    execute_actions(
        actions,
        &*state.rodio,
        &state.db,
        &state.player_state,
        &state.sinks,
    );
    Ok(())
}

/// FIX: Eliminates TOCTOU race — comparison + toggle under a single lock.
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
    persist_shuffle(state, new_shuffle);
    Ok(())
}

pub fn queue_set_repeat(state: &AppState, mode: RepeatMode) -> Result<(), AppError> {
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
    persist_repeat(state, new_mode);
    Ok(())
}

/// Fire-and-forget settings.json write for shuffle. Runs on the tokio
/// blocking pool so the calling thread (the `wire_sync!` spawn) returns
/// immediately. A failed write logs but does not surface — settings
/// persistence is best-effort, the in-memory state already changed.
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

pub async fn queue_load(state: &AppState) -> Result<(), AppError> {
    restore_persisted_queue(state).await
}

/// Shared helper used by both `queue_load` and the startup orchestration.
/// Loads the persisted queue from disk and restores it into `PlayerState`.
///
/// `repeat_mode` and `shuffle_enabled` are sourced from `settings.json`,
/// not `queue.json`. When a non-empty queue is restored, `shuffle_enabled`
/// is force-overridden to `false` (and `settings.json` is synced to match)
/// because `original_order` is not persisted — leaving shuffle on would
/// make "unshuffle" a no-op against an unknown original sequence.
pub async fn restore_persisted_queue(state: &AppState) -> AppResult<()> {
    let persistable = load_queue_from_disk_sync(&state.paths);
    let (summaries, persistable) = match persistable {
        Some(p) => {
            let summaries: Vec<Arc<TrackSummary>> =
                queries::track::get_track_summaries_by_ids(&state.db, &p.track_ids)
                    .await?
                    .into_iter()
                    .map(Arc::new)
                    .collect();
            (summaries, Some(p))
        }
        None => (Vec::new(), None),
    };

    // Single FS round-trip: read settings, capture repeat/shuffle, and
    // conditionally rewrite `shuffle_enabled = false` if a non-empty queue
    // was restored — all under the same `MUTATE_LOCK` so no other writer
    // can interleave between read and write.
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
        .map_err(|e| AppError::Settings(format!("restore_persisted_queue join: {e}")))??;

    with_state_emit(&state.player_state, &state.sinks, |s| {
        if let Some(p) = persistable.as_ref() {
            restore_queue(s, summaries, p);
        }
        s.queue.repeat_mode = settings_repeat;
        // Force shuffle off when a non-empty queue is restored — see doc.
        s.queue.shuffle_enabled = if restored_non_empty {
            false
        } else {
            settings_shuffle
        };
    });

    Ok(())
}

/// Shuffle the queue's `play_order` in place using a thread-local RNG,
/// pinning the currently-playing track to position 0 so playback stays
/// continuous. Allocation-free — no intermediate index Vec.
fn shuffle_inline(state: &mut crate::player::state::PlayerState) {
    let mut rng = rand::rng();
    state
        .queue
        .shuffle_play_order_in_place(&mut rng, /* anchor_to_current */ true);
}

#[cfg(test)]
#[path = "tests/queue_tests.rs"]
mod tests;
