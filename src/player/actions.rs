use std::collections::VecDeque;
use std::path::Path;

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;

use super::event_sink::PlayerSinks;
use super::rodio_backend::PlayerBackend;
use super::state::{
    PlayerAction, PlayerStateHandle, play_track_inner, stop_end_of_queue, with_state_emit,
};
use super::types::PersistableQueue;

/// Execute a list of `PlayerActions` against the audio backend and database.
/// Called after releasing the `PlayerState` lock.
///
/// `PlayMedia` is pre-flighted with `Path::exists()` and any decode failure
/// also auto-skips: the queue's `build_next_actions` is computed inline and
/// appended to the pending action set so a single stale double-click within
/// the watcher debounce window doesn't dead-end playback. The bad track
/// stays in the queue until the watcher catches up and `tasks::queue_prune`
/// removes it.
pub fn execute_actions<B: PlayerBackend>(
    actions: Vec<PlayerAction>,
    rodio_player: &B,
    db: &DbPool,
    paths: &Paths,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
) {
    let mut pending: VecDeque<PlayerAction> = actions.into();
    while let Some(action) = pending.pop_front() {
        match action {
            PlayerAction::PlayMedia {
                file_path,
                volume,
                speed,
                start_position_ms,
            } => {
                if !Path::new(&file_path).exists() {
                    log::warn!("Skipping vanished file: {file_path}");
                    rodio_player.stop();
                    enqueue_auto_skip(&mut pending, player_state, sinks);
                    continue;
                }
                if let Err(e) =
                    rodio_player.play_media(&file_path, volume, speed, start_position_ms)
                {
                    log::error!("Failed to play {file_path}: {e}");
                    rodio_player.stop();
                    enqueue_auto_skip(&mut pending, player_state, sinks);
                }
            }
            PlayerAction::Resume => rodio_player.resume(),
            PlayerAction::Pause => rodio_player.pause(),
            PlayerAction::Stop => rodio_player.stop(),
            PlayerAction::Seek { position_ms } => {
                rodio_player.seek(position_ms);
            }
            PlayerAction::SetVolume(v) => rodio_player.set_volume(v),
            PlayerAction::SetSpeed(s) => rodio_player.set_speed(s),
            PlayerAction::PreloadGapless(path) => {
                rodio_player.preload_gapless(path.as_deref());
            }
            PlayerAction::UpdatePlayCount(track_id) => {
                use crate::tasks::play_count_flusher::{PlayCountEvent, try_send};
                if !try_send(PlayCountEvent::Play(track_id)) {
                    // Flusher not installed (test contexts): fall back to a
                    // direct UPDATE so test invariants still hold.
                    let db = db.clone();
                    tokio::spawn(async move {
                        if let Err(e) = queries::track::update_play_count(&db, track_id).await {
                            log::warn!("Failed to update play count for {track_id}: {e}");
                        }
                    });
                }
            }
            PlayerAction::UpdateSkipCount(track_id) => {
                use crate::tasks::play_count_flusher::{PlayCountEvent, try_send};
                if !try_send(PlayCountEvent::Skip(track_id)) {
                    let db = db.clone();
                    tokio::spawn(async move {
                        if let Err(e) = queries::track::update_skip_count(&db, track_id).await {
                            log::warn!("Failed to update skip count for {track_id}: {e}");
                        }
                    });
                }
            }
            PlayerAction::SavePosition {
                track_id,
                position_ms,
            } => {
                let db = db.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        queries::track::update_last_position(
                            &db,
                            track_id,
                            i64::try_from(position_ms).unwrap_or(i64::MAX),
                        )
                        .await
                    {
                        log::warn!("Failed to save position for {track_id}: {e}");
                    }
                });
            }
            PlayerAction::SaveQueue(queue) => {
                save_queue_to_disk_sync(paths, &queue);
            }
        }
    }
}

/// Advance past a track that turned out to be unplayable (file missing or
/// decode error) and append the resulting actions to the pending set.
/// On an empty queue this emits `Stop` only — the `with_state_emit` call
/// took the lock to do that, so the state machine and `ViewModel` both reflect
/// the end-of-queue state.
fn enqueue_auto_skip(
    pending: &mut VecDeque<PlayerAction>,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
) {
    let actions = with_state_emit(player_state, sinks, |s| {
        if let Some(track) = s.queue.advance_skip().cloned() {
            play_track_inner(s, track, None)
        } else {
            stop_end_of_queue(s)
        }
    });
    // Walk the new actions in order so they keep their relative ordering
    // ahead of anything still queued from the original batch.
    for (i, a) in actions.into_iter().enumerate() {
        pending.insert(i, a);
    }
}

fn save_queue_to_disk_sync(paths: &Paths, queue: &PersistableQueue) {
    if let Err(e) = crate::services::write_json_atomic_sync(&paths.queue_path, queue) {
        log::error!("Failed to write queue file: {e}");
    }
}

#[cfg(test)]
#[path = "tests/actions_tests.rs"]
mod tests;
