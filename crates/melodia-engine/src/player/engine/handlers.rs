use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::actions::emit_and_execute;
use super::backend::{PlaybackCheck, PlaybackEngine};
use super::event_sink::PlayerSinks;
use super::state::{
    PlayerAction, PlayerState, PlayerStateHandle, PositionTick, lock_state, with_state_emit,
};
use super::types::{PersistedPlayback, PlaybackSource, PlaybackStatus};
use melodia_audio::player::source::prebuffer::StreamShared;
use melodia_playback::player::playback::crossfade;
use melodia_playback::player::playback::replaygain::TrackReplayGain;

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

/// How much playback a `SIGKILL` may cost: the queue-and-position snapshot lands this often
/// while something is playing, and `main.rs`'s shutdown hook writes the authoritative one on
/// a clean exit.
///
/// Measured in playback rather than wall time, the counter below sitting past every arm of the
/// loop that skips a tick.
const SAVE_INTERVAL_MS: u64 = 30_000;

/// Polls spanning [`SAVE_INTERVAL_MS`]. Derived rather than spelled, so the interval its own
/// comment argues cannot drift from the count the loop actually applies.
const SAVE_EVERY_N_TICKS: u64 = SAVE_INTERVAL_MS / POLL_INTERVAL_MS;

const _: () = assert!(
    SAVE_EVERY_N_TICKS > 0,
    "the save cadence must span at least one poll, or its modulus is zero"
);

/// How often the OS media controls are handed a position. Their widgets advance on their own
/// between updates, so this only has to keep the two from visibly diverging.
#[cfg(any(target_os = "windows", target_os = "macos"))]
const MEDIA_POSITION_INTERVAL_MS: u64 = 5_000;

/// Polls spanning [`MEDIA_POSITION_INTERVAL_MS`], for [`SAVE_EVERY_N_TICKS`]' reason.
#[cfg(any(target_os = "windows", target_os = "macos"))]
const MEDIA_POSITION_EVERY_N_TICKS: u64 = MEDIA_POSITION_INTERVAL_MS / POLL_INTERVAL_MS;

#[cfg(any(target_os = "windows", target_os = "macos"))]
const _: () = assert!(
    MEDIA_POSITION_EVERY_N_TICKS > 0,
    "the media-controls cadence must span at least one poll, or its modulus is zero"
);

/// Rate limiter on the position publish, admitting one tick per whole second.
///
/// The monitor wakes at [`POLL_INTERVAL_MS`] so the crossfade and gapless windows stay tight,
/// but the now-playing bar renders seconds and a slider thumb on a long track moves invisibly
/// between two 500 ms samples. Publishing at the poll rate makes the UI rebuild text content
/// twice per visible change, so the tick nobody can see is the one worth dropping.
///
/// A type rather than a local, because the seed is the half worth holding and a bare `&mut u64`
/// leaves it at whichever call site constructs one.
struct SecondGate(u64);

impl Default for SecondGate {
    /// Past any real position, so the first tick of a freshly started track publishes instead
    /// of waiting for its first second boundary.
    fn default() -> Self {
        Self(u64::MAX)
    }
}

impl SecondGate {
    /// Whether `position_ms` lands in a second the UI has not been shown, recording it either
    /// way. A seek backwards admits: what matters is that the second moved, not which way.
    fn admits(&mut self, position_ms: u64) -> bool {
        let second = position_ms / 1000;
        let moved = second != self.0;
        self.0 = second;
        moved
    }
}

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

    // A live source has no track end, which is the only thing the two decisions below are about:
    // a crossfade ramps between two tracks and a gapless preload stages the next one. The position
    // published above is elapsed listening time, since the silence the prebuffer emits while
    // starved still advances the deck's clock.
    if !state.source_allows(PlaybackSource::advances_queue) {
        return Some(PlayingTick {
            tick,
            late_preload: None,
            crossfade: None,
        });
    }

    let next = state.queue.peek_next();
    let same_album = match (state.current_track(), next) {
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
        track_id: state.current_track().map(|t| t.id),
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
        // A deck reads 0 until it has been pulled for the source just started on
        // it, and a track shorter than the lead would read its whole length as
        // remaining and stage a preload on the spot.
        && state.position_ms > 0
        && state.position_ms <= state.duration_ms
        && state.duration_ms.saturating_sub(state.position_ms) < PRELOAD_LEAD_MS
    {
        // Capture the next track's baked ReplayGain alongside its path — it must
        // travel with *its own* source (the preloaded track has different tags
        // than the playing one), so the gain is baked per source, not shared.
        state.queue.peek_next().map(|t| (t.file_path.clone(), t.as_ref().into()))
    } else {
        None
    };

    Some(PlayingTick {
        tick,
        late_preload,
        crossfade,
    })
}

/// Tell the user a station gave up, which is otherwise a silence with no explanation.
///
/// Named rather than described: the station is what they chose, and by the time this fires the
/// state is about to forget it.
fn notify_station_ended(player_state: &PlayerStateHandle) {
    let station = lock_state(player_state).station().map(|s| s.name.clone());
    if let Some(name) = station {
        melodia_core::utils::toast::notify(
            melodia_core::utils::toast::ToastKind::PlaybackFailed,
            name,
        );
    }
}

/// Bring `PlayerState` back in line with what the live stream is actually doing, emitting only
/// when something moved.
///
/// The buffering flag and the ICY title are the two things a station changes on its own, and both
/// arrive on the feed thread with no way to reach the state lock from there. Polling them on the
/// tick the monitor already runs is cheaper than a channel and a task per station; the change
/// checks are what stop it republishing the view model twice a second for a station that is
/// perfectly happy.
fn reconcile_live_stream(
    stream: &StreamShared,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    last_title_generation: &mut u64,
) {
    let buffering = stream.is_buffering();
    let title_generation = stream.title_generation();
    let title_moved = title_generation != *last_title_generation;
    let buffering_moved =
        lock_state(player_state).station().is_some_and(|station| station.buffering != buffering);

    if !title_moved && !buffering_moved {
        return;
    }
    *last_title_generation = title_generation;
    let title = title_moved.then(|| stream.title());

    with_state_emit(player_state, sinks, |state| {
        if let Some(radio) = state.station_mut() {
            // The one place the station is mutated in flight, so the `Arc`'s copy-on-write lands
            // here: once a song, against a clone per emit if the struct were held inline.
            let radio = std::sync::Arc::make_mut(radio);
            radio.buffering = buffering;
            if let Some(title) = title {
                radio.live_title = title;
            }
        }
        Vec::<PlayerAction>::new()
    });
}

/// What the monitor knows at a save point, for whoever owns the writing.
pub struct PlaybackSnapshot {
    /// The playing track and how far into it, `None` when nothing is.
    pub track: Option<(i64, u64)>,
    pub playback: PersistedPlayback,
}

/// How the monitor hands a snapshot over.
///
/// A sink rather than a `DbPool` and a `Paths`: what the engine knows is where playback got to,
/// and *where that is written down* is a question one layer up. Awaited inline at the call site,
/// so an in-flight save still completes before shutdown wins the next select.
pub type SnapshotSink =
    Arc<dyn Fn(PlaybackSnapshot) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// All long-lived handles the playback monitor needs to operate. Bundled
/// so `spawn_playback_monitor` doesn't accumulate a long argument list as
/// the monitor's responsibilities grow.
pub struct PlaybackMonitorContext {
    pub shutdown_token: CancellationToken,
    pub player_state: Arc<PlayerStateHandle>,
    pub engine: Arc<PlaybackEngine>,
    pub sinks: Arc<PlayerSinks>,
    pub position_tx: watch::Sender<Option<PositionTick>>,
    pub save: SnapshotSink,
}

/// Spawns a single background task that handles position polling,
/// gapless transition detection, and end-of-stream detection.
pub fn spawn_playback_monitor(tracker: &TaskTracker, ctx: PlaybackMonitorContext) {
    let PlaybackMonitorContext {
        shutdown_token,
        player_state,
        engine,
        sinks,
        position_tx,
        save,
    } = ctx;
    tracker.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let mut media_tick_counter: u64 = 0;

        let mut save_tick_counter: u64 = 0;
        let mut publish = SecondGate::default();

        // Last ICY title generation reconciled into `PlayerState`. Generations are process-wide
        // tickets starting at 1, so this holds across stations and `0` means nothing seen yet.
        let mut last_title_generation: u64 = 0;

        loop {
            tokio::select! {
                biased;
                () = shutdown_token.cancelled() => {
                    log::info!("Playback monitor stopped");
                    break;
                }
                _ = interval.tick() => {}
            }

            // Ahead of the not-playing short circuit below, because a stop is what retires the
            // most sources at once and it lands on exactly the ticks that circuit skips.
            engine.collect_spent();

            // Quick check: skip tick when not playing (lock-free via atomic mirror)
            let is_playing = player_state.status_atomic.load(std::sync::atomic::Ordering::Relaxed)
                == PlaybackStatus::Playing as u8;

            if !is_playing {
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                {
                    media_tick_counter = 0;
                }
                continue;
            }

            // Single lock acquisition to avoid TOCTOU between gapless and EOS checks
            match engine.check_playback_state() {
                PlaybackCheck::GaplessTransition => {
                    emit_and_execute(&*engine, &player_state, &sinks, |state| {
                        let mut actions = Vec::with_capacity(2);

                        // Update play count for the track that just finished
                        if let Some(track) = state.current_track() {
                            actions.push(PlayerAction::UpdatePlayCount(track.id));
                        }

                        // Advance queue — update state only (the deck is already playing).
                        // The next gapless preload is staged later, by the `Playing`
                        // branch, when this new current track approaches its own end.
                        if let Some(track) = state.queue.advance().cloned() {
                            state.position_ms = 0;
                            state.duration_ms =
                                u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
                            state.source = Some(PlaybackSource::Track(track));
                        }

                        actions
                    });
                }
                PlaybackCheck::EndOfStream => {
                    // A station's deck drains for exactly one reason — its feed thread spent the
                    // reconnect budget — and `is_finished` is that reason. Anything else is a deck
                    // caught in the instant between `play_stream` publishing the cell and
                    // appending the source, where an empty deck means "not yet", not "over".
                    let live = engine.stream_shared();
                    if live.as_deref().is_some_and(|s| !s.is_finished()) {
                        continue;
                    }
                    if live.is_some() {
                        notify_station_ended(&player_state);
                    }
                    // Advance the queue (or, if the sleep-timer's "End of current
                    // track" mode is armed, disarm it and stop instead). See
                    // `PlayerState::build_end_of_stream_actions`.
                    emit_and_execute(
                        &*engine,
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
                        position_ms: engine.query_position(),
                        already_preloaded: engine.is_gapless_preloaded(),
                        crossfading: engine.is_crossfading(),
                        xf: engine.crossfade_settings(),
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

                    if publish.admits(tick.position_ms) {
                        let _ = position_tx.send(Some(tick.clone()));
                    }

                    if let Some(stream) = engine.stream_shared() {
                        reconcile_live_stream(
                            &stream,
                            &player_state,
                            &sinks,
                            &mut last_title_generation,
                        );
                    }

                    if let Some(decision) = crossfade_now {
                        // Advance the queue and start the incoming track on the
                        // idle deck in one serialized step. `emit_and_execute`
                        // re-reads the queue *and* re-verifies the status, the
                        // current track and the position under the exec lock, so
                        // anything that landed since the decision above can't be
                        // clobbered.
                        emit_and_execute(&*engine, &player_state, &sinks, |state| {
                            state.build_crossfade_actions(decision)
                        });
                    } else if let Some((path, rg)) = late_preload {
                        engine.preload_gapless(Some(&path), rg);
                    }

                    #[cfg(any(target_os = "windows", target_os = "macos"))]
                    {
                        media_tick_counter =
                            (media_tick_counter + 1) % MEDIA_POSITION_EVERY_N_TICKS;
                        if media_tick_counter == 0
                            && let Some(mc) = sinks.media_controls.as_ref()
                        {
                            mc.update_position(tick.position_ms);
                        }
                    }
                }
            }

            // Below every arm that skips a tick, so the cadence counts playback rather than wall
            // time. Snapshots under the state lock and awaits the sink inline — the monitor task
            // itself is tracked by `tracker`, so an in-flight save completes before shutdown
            // wins on the next select.
            save_tick_counter = (save_tick_counter + 1) % SAVE_EVERY_N_TICKS;
            if save_tick_counter == 0 {
                let snapshot = {
                    let state = lock_state(&player_state);
                    PlaybackSnapshot {
                        track: state.current_track().map(|t| (t.id, state.position_ms)),
                        playback: state.to_persisted(),
                    }
                };
                save(snapshot).await;
            }
        }
    });
}

#[cfg(test)]
#[path = "tests/handlers_tests.rs"]
mod tests;
