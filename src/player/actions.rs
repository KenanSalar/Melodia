use std::collections::VecDeque;
use std::path::Path;

use crate::database::DbPool;
use crate::database::queries;

use super::event_sink::PlayerSinks;
use super::replaygain::TrackReplayGain;
use super::rodio_backend::PlayerBackend;
use super::state::{
    PlayerAction, PlayerState, PlayerStateHandle, play_track_inner, stop_end_of_queue,
    with_state_emit,
};

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
                replaygain,
            } => {
                if !Path::new(&file_path).exists() {
                    log::warn!("Skipping vanished file: {file_path}");
                    rodio_player.stop();
                    enqueue_auto_skip(&mut pending, player_state, sinks);
                    continue;
                }
                if let Err(e) =
                    rodio_player.play_media(&file_path, volume, speed, start_position_ms, replaygain)
                {
                    log::error!("Failed to play {file_path}: {e}");
                    // Surface the failure — the music silently stopping is
                    // otherwise invisible. (The vanished-file branch above stays
                    // silent: it auto-recovers by skipping to the next track.)
                    let name = Path::new(&file_path)
                        .file_name()
                        .map_or_else(|| file_path.clone(), |n| n.to_string_lossy().into_owned());
                    crate::services::toast::notify(
                        crate::services::toast::ToastKind::PlaybackFailed,
                        name,
                    );
                    rodio_player.stop();
                    enqueue_auto_skip(&mut pending, player_state, sinks);
                }
            }
            PlayerAction::BeginCrossfade {
                file_path,
                replaygain,
                fade_ms,
                volume,
                speed,
            } => {
                // Same pre-flight + auto-skip contract as `PlayMedia`, with one
                // difference: no `rodio_player.stop()`. The outgoing track is
                // still audible on the other deck, and the `play_media` that
                // `enqueue_auto_skip` produces takes over from it cleanly either
                // way — hard-cutting (it clears both decks) or, with
                // `crossfade_manual` on, fading out of it. Stopping here would
                // only insert a gap of silence ahead of that.
                if !Path::new(&file_path).exists() {
                    log::warn!("Skipping vanished file at crossfade: {file_path}");
                    enqueue_auto_skip(&mut pending, player_state, sinks);
                    continue;
                }
                if let Err(e) =
                    rodio_player.begin_crossfade(&file_path, replaygain, fade_ms, volume, speed)
                {
                    log::error!("Failed to crossfade into {file_path}: {e}");
                    let name = Path::new(&file_path)
                        .file_name()
                        .map_or_else(|| file_path.clone(), |n| n.to_string_lossy().into_owned());
                    crate::services::toast::notify(
                        crate::services::toast::ToastKind::PlaybackFailed,
                        name,
                    );
                    // `build_crossfade_actions` already advanced onto this track,
                    // so `advance_skip` correctly lands on the one after it. In
                    // `RepeatMode::One` that also steps off the repeated track —
                    // rare enough (the file vanished mid-play) to accept.
                    enqueue_auto_skip(&mut pending, player_state, sinks);
                }
            }
            PlayerAction::Resume => rodio_player.resume(),
            PlayerAction::Pause { fade_ms } => rodio_player.pause_with_fade(fade_ms),
            PlayerAction::Stop { fade_ms } => rodio_player.stop_with_fade(fade_ms),
            PlayerAction::Seek { position_ms } => {
                rodio_player.seek(position_ms);
            }
            PlayerAction::SetVolume(v) => rodio_player.set_volume(v),
            PlayerAction::SetSpeed(s) => rodio_player.set_speed(s),
            PlayerAction::PreloadGapless(path) => {
                // This action only ever *clears* a stale preload (`path` is
                // `None`); the real gapless preload with baked ReplayGain happens
                // directly in `spawn_playback_monitor`. A default (unity) RG here
                // is harmless — the `None` path never builds an audio source.
                rodio_player.preload_gapless(path.as_deref(), TrackReplayGain::default());
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
        }
    }
}

/// Mutate state, publish the `ViewModel`, then execute the resulting side
/// effects — all while holding the per-`PlayerStateHandle` execution lock so
/// mutation order equals side-effect order across tokio workers.
///
/// This is the serialized replacement for the bare `with_state_emit(...)` +
/// `execute_actions(...)` pair. `with_state_emit` alone keeps each *mutation*
/// atomic, but the `execute_actions` that follows runs on whatever worker the
/// caller is on; two batches from different tasks (e.g. the playback monitor's
/// EOS-advance and a UI `Stop`) could otherwise interleave their rodio effects
/// and leave state and backend disagreeing (a rare TOCTOU). Holding `exec_lock`
/// across both halves closes that window.
///
/// The lock spans only synchronous work (no `.await`), so a blocking mutex is
/// correct. `enqueue_auto_skip`'s nested `with_state_emit` inside
/// `execute_actions` takes the *state* mutex, never `exec_lock`, so there is no
/// re-entrancy.
pub fn emit_and_execute<B, F>(
    rodio_player: &B,
    db: &DbPool,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    f: F,
) where
    B: PlayerBackend,
    F: FnOnce(&mut PlayerState) -> Vec<PlayerAction>,
{
    let _exec = player_state.lock_exec();
    let actions = with_state_emit(player_state, sinks, f);
    execute_actions(actions, rodio_player, db, player_state, sinks);
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

#[cfg(test)]
#[path = "tests/actions_tests.rs"]
mod tests;
