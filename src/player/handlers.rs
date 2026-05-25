use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;

use super::actions::execute_actions;
use super::event_sink::PlayerSinks;
use super::rodio_backend::{PlaybackCheck, RodioPlayer};
use super::state::{
    PlayerAction, PlayerStateHandle, PositionTick, lock_state, play_track_inner,
    stop_end_of_queue, with_state_emit,
};
use super::types::PlaybackStatus;

/// How many ms before the end of the current track we stage the next gapless
/// source. Generous enough that the file can decode and queue before EOS even
/// on a slow disk, tight enough that mid-track repeat-mode / queue changes
/// have a chance to influence what gets preloaded.
const PRELOAD_LEAD_MS: u64 = 1500;

/// All long-lived handles the playback monitor needs to operate. Bundled
/// so `spawn_playback_monitor` doesn't accumulate a long argument list as
/// the monitor's responsibilities grow.
pub struct PlaybackMonitorContext {
    pub shutdown_token: CancellationToken,
    pub player_state: Arc<PlayerStateHandle>,
    pub rodio_player: Arc<RodioPlayer>,
    pub sinks: Arc<PlayerSinks>,
    pub position_tx: watch::Sender<Option<PositionTick>>,
    pub db: DbPool,
    pub paths: Paths,
}

/// Spawns a single background task that handles position polling,
/// gapless transition detection, and end-of-stream detection.
pub fn spawn_playback_monitor(tracker: &TaskTracker, ctx: PlaybackMonitorContext) {
    let PlaybackMonitorContext {
        shutdown_token,
        player_state,
        rodio_player,
        sinks,
        position_tx,
        db,
        paths,
    } = ctx;
    tracker.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));

        // Tick counter for OS media controls periodic position updates (~5s = every 10th tick)
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let mut media_tick_counter: u32 = 0;

        // Periodic queue+position save: every 60 *playing* ticks (~30s).
        // Survives SIGKILL/crash; the authoritative final snapshot still
        // happens in main.rs's shutdown hook on clean exit.
        let mut save_tick_counter: u32 = 0;

        // Last whole-second value sent to the UI. The playback monitor wakes
        // every 500 ms (so gapless preload triggers and EOS detection stay
        // tight), but the now-playing bar's time labels render seconds and
        // the slider thumb on a 1 h+ track moves invisibly between 500 ms
        // ticks — publishing at 2 Hz forces the UI to repaint with new text
        // content twice per visible change. Rate-limiting to 1 Hz here
        // halves both the renderer's per-tick cache churn and the
        // property-binding dirty-mark traffic. Initialised to `u64::MAX` so
        // the first publish after playback starts always fires (no
        // `if last_published_second == position_seconds` skip on the first
        // sample of a freshly-started track).
        let mut last_published_second: u64 = u64::MAX;

        loop {
            tokio::select! {
                biased;
                () = shutdown_token.cancelled() => {
                    log::info!("Playback monitor stopped");
                    break;
                }
                _ = interval.tick() => {}
            }

            // Quick check: skip tick when not playing (lock-free via atomic mirror)
            let is_playing = player_state
                .status_atomic
                .load(std::sync::atomic::Ordering::Relaxed)
                == PlaybackStatus::Playing as u8;

            if !is_playing {
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                {
                    media_tick_counter = 0;
                }
                continue;
            }

            // Single lock acquisition to avoid TOCTOU between gapless and EOS checks
            match rodio_player.check_playback_state() {
                PlaybackCheck::GaplessTransition => {
                    let actions = with_state_emit(&player_state, &sinks, |state| {
                        let mut actions = Vec::with_capacity(2);

                        // Update play count for the track that just finished
                        if let Some(ref track) = state.current_track {
                            actions.push(PlayerAction::UpdatePlayCount(track.id));
                        }

                        // Advance queue — update state only (Rodio is already playing).
                        // The next gapless preload is staged later, by the `Playing`
                        // branch, when this new current track approaches its own end.
                        if let Some(track) = state.queue.advance().cloned() {
                            state.position_ms = 0;
                            state.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
                            state.current_track = Some(track);
                        }

                        actions
                    });

                    execute_actions(actions, &rodio_player, &db, &paths, &player_state, &sinks);
                }
                PlaybackCheck::EndOfStream => {
                    let actions = with_state_emit(&player_state, &sinks, |state| {
                        let mut actions = Vec::with_capacity(4);

                        // Update play count for the track that just ended
                        if let Some(ref track) = state.current_track {
                            actions.push(PlayerAction::UpdatePlayCount(track.id));
                        }

                        // Advance queue
                        if let Some(track) = state.queue.advance().cloned() {
                            actions.extend(play_track_inner(state, track, None));
                        } else {
                            actions.extend(stop_end_of_queue(state));
                        }

                        actions
                    });

                    execute_actions(actions, &rodio_player, &db, &paths, &player_state, &sinks);
                }
                PlaybackCheck::Playing => {
                    // Normal tick: update position with lightweight event
                    // Query position BEFORE locking PlayerState to avoid nested lock
                    let pos = rodio_player.query_position();
                    let already_preloaded = rodio_player.is_gapless_preloaded();
                    let (tick, late_preload) = {
                        let mut state = lock_state(&player_state);
                        if state.status != PlaybackStatus::Playing {
                            continue;
                        }
                        state.position_ms = pos;
                        let tick = PositionTick {
                            position_ms: pos,
                            duration_ms: state.duration_ms,
                        };

                        // Late gapless preload: stage the next track only when the
                        // current one is within PRELOAD_LEAD_MS of ending. Doing it
                        // late lets mid-track repeat-mode / queue mutations decide
                        // what plays next (eager preload would lock in a stale
                        // choice the moment the current track started).
                        let late_preload = if state.gapless_enabled
                            && !already_preloaded
                            && state.duration_ms > 0
                            && state.duration_ms.saturating_sub(state.position_ms)
                                < PRELOAD_LEAD_MS
                        {
                            state.queue.peek_next().map(|t| t.file_path.clone())
                        } else {
                            None
                        };

                        (tick, late_preload)
                    };

                    // Publish to the UI only when the whole-second has changed
                    // (1 Hz effective rate). The 500 ms loop cadence is kept
                    // for gapless / EOS detection and the late-preload check
                    // above; the UI side reads `Player.position-ms` which only
                    // drives second-resolution displays. See the comment on
                    // `last_published_second` above.
                    let position_seconds = tick.position_ms / 1000;
                    if position_seconds != last_published_second {
                        last_published_second = position_seconds;
                        let _ = position_tx.send(Some(tick.clone()));
                    }

                    if let Some(path) = late_preload {
                        rodio_player.preload_gapless(Some(&path));
                    }

                    // OS media controls: update position every ~5 seconds
                    #[cfg(any(target_os = "windows", target_os = "macos"))]
                    {
                        media_tick_counter = (media_tick_counter + 1) % 10;
                        if media_tick_counter == 0
                            && let Some(mc) = sinks.media_controls.as_ref()
                        {
                            mc.update_position(tick.position_ms);
                        }
                    }
                }
            }

            // Periodic save (every 60 playing ticks ~= 30s). Snapshots the
            // current track id + position and the queue, then awaits the DB
            // and FS writes inline — the monitor task itself is tracked by
            // `tracker`, so an in-flight save completes before shutdown wins
            // on the next select.
            save_tick_counter = (save_tick_counter + 1) % 60;
            if save_tick_counter == 0 {
                let (track_data, persistable) = {
                    let state = lock_state(&player_state);
                    let td = state
                        .current_track
                        .as_ref()
                        .map(|t| (t.id, state.position_ms));
                    (td, state.queue.to_persistable())
                };
                if let Some((track_id, position_ms)) = track_data
                    && let Err(e) = queries::track::update_last_position(
                        &db,
                        track_id,
                        i64::try_from(position_ms).unwrap_or(i64::MAX),
                    )
                    .await
                {
                    log::warn!("periodic save: update_last_position {track_id}: {e}");
                }
                let queue_path = paths.queue_path.clone();
                let join = tokio::task::spawn_blocking(move || {
                    crate::services::write_json_atomic_sync(&queue_path, &persistable)
                })
                .await;
                match join {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => log::warn!("periodic save: write queue.json: {e}"),
                    Err(e) => log::warn!("periodic save: spawn_blocking: {e}"),
                }
            }
        }
    });
}
