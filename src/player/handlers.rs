use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;

use super::actions::emit_and_execute;
use super::crossfade;
use super::event_sink::PlayerSinks;
use super::replaygain::TrackReplayGain;
use super::rodio_backend::{PlaybackCheck, RodioPlayer};
use super::state::{
    PlayerAction, PlayerState, PlayerStateHandle, PositionTick, lock_state,
};
use super::types::PlaybackStatus;

/// How often the monitor wakes: tight enough that gapless preload triggers and
/// end-of-stream detection stay responsive, loose enough not to spin.
const POLL_INTERVAL_MS: u64 = 500;

/// The crossfade trigger window is `[MIN_FADE_MS, configured duration]`, and the
/// monitor only samples it once per poll. At the shortest configurable duration
/// it must still be **at least one poll wide**, or a tick can step straight over
/// it (e.g. 700 ms remaining → 200 ms remaining) and no crossfade fires. That is
/// not a benign miss: `crossfade_eligible` has already suppressed the gapless
/// preload by then, so the transition degrades to a decode-and-start hard cut —
/// an audible gap, worse than the gapless behaviour crossfade replaced.
const _: () = assert!(
    crossfade::MIN_CROSSFADE_MS as u64 >= crossfade::MIN_FADE_MS + POLL_INTERVAL_MS,
    "the shortest crossfade must leave a trigger window at least one poll wide"
);

/// How many ms before the end of the current track we stage the next gapless
/// source. Generous enough that the file can decode and queue before EOS even
/// on a slow disk, tight enough that mid-track repeat-mode / queue changes
/// have a chance to influence what gets preloaded.
const PRELOAD_LEAD_MS: u64 = 1500;

/// What the monitor read off the audio backend before taking the `PlayerState`
/// lock. Gathered first, deliberately: querying the backend under the state lock
/// would nest the decks mutex inside it.
#[derive(Copy, Clone)]
pub struct BackendSnapshot {
    pub position_ms: u64,
    pub already_preloaded: bool,
    pub crossfading: bool,
    pub xf: crossfade::CrossfadeSettings,
}

/// What one `Playing` tick decided: the position to publish, and at most one of
/// a crossfade or a gapless preload — never both, since they are two ways to
/// make the *same* transition.
pub struct PlayingTick {
    pub tick: PositionTick,
    /// Path + baked `ReplayGain` of the track to stage behind the current one.
    pub late_preload: Option<(String, TrackReplayGain)>,
    pub crossfade: Option<crossfade::CrossfadeDecision>,
}

/// The whole of a `Playing` tick's decision, as a pure function of the state and
/// what the backend reported. `None` means the tick is void — playback moved off
/// `Playing` between the backend reads and the lock, so the caller skips it.
///
/// Split out of the monitor loop so the crossfade-vs-gapless gate can be tested
/// directly, without a running audio backend.
pub fn evaluate_playing_tick(
    state: &mut PlayerState,
    backend: BackendSnapshot,
) -> Option<PlayingTick> {
    let BackendSnapshot {
        position_ms,
        already_preloaded,
        crossfading,
        xf,
    } = backend;

    if state.status != PlaybackStatus::Playing {
        return None;
    }
    state.position_ms = position_ms;
    let tick = PositionTick {
        position_ms,
        duration_ms: state.duration_ms,
    };

    let next = state.queue.peek_next();
    let same_album = match (state.current_track.as_ref(), next) {
        (Some(cur), Some(nxt)) => crossfade::same_album(cur, nxt),
        _ => false,
    };
    // Timing-INDEPENDENT: does this transition belong to the crossfade path at
    // all? The gapless preload below is gated on its negation, and it must not
    // depend on the position — a crossfade shorter than PRELOAD_LEAD_MS would
    // otherwise let the preload fire first, set `gapless_pending`, and
    // permanently block the crossfade via its own gate.
    let eligible = crossfade::crossfade_eligible(
        xf,
        state.pause_after_current_track,
        next.is_some(),
        same_album,
    );

    // Carry the state this decision was made against. The caller holds the
    // `PlayerState` lock now but takes `exec_lock` only later, so a pause / stop
    // / seek / manual track change can land in between; `build_crossfade_actions`
    // re-verifies the whole snapshot under the emit lock and bails if any of it
    // moved.
    let crossfade = crossfade::should_crossfade(
        eligible,
        already_preloaded,
        crossfading,
        state.position_ms,
        state.duration_ms,
        xf.duration_ms,
    )
    .map(|fade_ms| crossfade::CrossfadeDecision {
        fade_ms,
        track_id: state.current_track.as_ref().map(|t| t.id),
        position_ms: state.position_ms,
    });

    // Late gapless preload: stage the next track only when the current one is
    // within PRELOAD_LEAD_MS of ending. Doing it late lets mid-track repeat-mode
    // / queue mutations decide what plays next (an eager preload would lock in a
    // stale choice the moment the current track started).
    let late_preload = if state.gapless_enabled
        && !already_preloaded
        // A crossfade runs the next track on the *other* deck. A gapless source
        // would sit on this one, behind the outgoing track, and inherit its fade
        // cell.
        && !eligible
        && !crossfading
        // Sleep-timer "End of current track": suppress the gapless preload so the
        // current track drains to `EndOfStream` (not `GaplessTransition`, which
        // would already be playing the next track) — that's the only boundary
        // `build_end_of_stream_actions`' pause-at-track-end gate can catch.
        && !state.pause_after_current_track
        && state.duration_ms > 0
        // Reject a stale position read from a deck rodio hasn't re-anchored yet
        // (it only zeroes the tracked position when `clear()` actually removes a
        // source).
        && state.position_ms > 0
        && state.position_ms <= state.duration_ms
        && state.duration_ms.saturating_sub(state.position_ms) < PRELOAD_LEAD_MS
    {
        // Capture the next track's baked ReplayGain alongside its path — it must
        // travel with *its own* source (the preloaded track has different tags
        // than the playing one), so the gain is baked per source, not shared.
        state
            .queue
            .peek_next()
            .map(|t| (t.file_path.clone(), t.replaygain()))
    } else {
        None
    };

    Some(PlayingTick {
        tick,
        late_preload,
        crossfade,
    })
}

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
        let mut interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));

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
                    emit_and_execute(&rodio_player, &db, &player_state, &sinks, |state| {
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
                }
                PlaybackCheck::EndOfStream => {
                    // Advance the queue (or, if the sleep-timer's "End of current
                    // track" mode is armed, disarm it and stop instead). See
                    // `PlayerState::build_end_of_stream_actions`.
                    emit_and_execute(
                        &rodio_player,
                        &db,
                        &player_state,
                        &sinks,
                        PlayerState::build_end_of_stream_actions,
                    );
                }
                PlaybackCheck::Playing => {
                    // Normal tick: update position with lightweight event.
                    // Query the backend BEFORE locking PlayerState to avoid a
                    // nested lock — `evaluate_playing_tick` takes these as inputs.
                    let backend = BackendSnapshot {
                        position_ms: rodio_player.query_position(),
                        already_preloaded: rodio_player.is_gapless_preloaded(),
                        crossfading: rodio_player.is_crossfading(),
                        xf: rodio_player.crossfade_settings(),
                    };
                    let decided = {
                        let mut state = lock_state(&player_state);
                        evaluate_playing_tick(&mut state, backend)
                    };
                    let Some(PlayingTick {
                        tick,
                        late_preload,
                        crossfade: crossfade_now,
                    }) = decided
                    else {
                        continue;
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

                    if let Some(decision) = crossfade_now {
                        // Advance the queue and start the incoming track on the
                        // idle deck in one serialized step. `emit_and_execute`
                        // re-reads the queue *and* re-verifies the status, the
                        // current track and the position under the exec lock, so
                        // anything that landed since the decision above can't be
                        // clobbered.
                        emit_and_execute(&rodio_player, &db, &player_state, &sinks, |state| {
                            state.build_crossfade_actions(decision)
                        });
                    } else if let Some((path, rg)) = late_preload {
                        rodio_player.preload_gapless(Some(&path), rg);
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

#[cfg(test)]
#[path = "tests/handlers_tests.rs"]
mod tests;
