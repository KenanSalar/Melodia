use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::actions::emit_and_execute;
use super::backend::{DeckClock, PlaybackCheck, PlaybackEngine};
use super::event_sink::PlayerSinks;
use super::signal_path::{SignalPath, Transport};
use super::state::{
    PlayerAction, PlayerState, PlayerStateHandle, PositionTick, ReleaseDecision, lock_state,
    with_state_emit,
};
use super::types::{PersistedPlayback, PlaybackSource, PlaybackStatus};
use melodia_audio::player::source::prebuffer::StreamShared;
use melodia_core::entities::track::TrackSummary;
use melodia_playback::player::playback::crossfade;

/// How often the monitor wakes: tight enough that gapless preload triggers and
/// end-of-stream detection stay responsive, loose enough not to spin.
const POLL_INTERVAL_MS: u64 = 500;

/// The crossfade trigger window, `[MIN_FADE_MS, duration]`, is sampled once a poll, so at its
/// shortest it must be a poll wide. A tick stepping over it fires no crossfade, and with the
/// gapless preload already suppressed the transition becomes a hard cut.
const _: () = assert!(
    crossfade::MIN_CROSSFADE_MS as u64 >= crossfade::MIN_FADE_MS + POLL_INTERVAL_MS,
    "the shortest crossfade must leave a trigger window at least one poll wide"
);

/// How far before the end the next gapless source is staged: long enough to decode on a slow
/// disk, short enough that a queue change mid-track still decides it.
const PRELOAD_LEAD_MS: u64 = 1500;

/// How much playback a `SIGKILL` may cost; a clean exit writes the authoritative snapshot.
/// Counted in playback rather than wall time, a skipped poll not counting.
const SAVE_INTERVAL_MS: u64 = 30_000;

/// Polls spanning [`SAVE_INTERVAL_MS`], derived so the two can't drift.
const SAVE_EVERY_N_TICKS: u64 = SAVE_INTERVAL_MS / POLL_INTERVAL_MS;

const _: () = assert!(
    SAVE_EVERY_N_TICKS > 0,
    "the save cadence must span at least one poll, or its modulus is zero"
);

/// How long a pause may hold an exclusive device before it goes back, where the user asked for
/// that. The Output card's row spells the figure in words.
const RELEASE_AFTER_PAUSE_MS: u64 = 5 * 60 * 1000;

const RELEASE_AFTER_N_TICKS: u64 = RELEASE_AFTER_PAUSE_MS / POLL_INTERVAL_MS;

/// Admits one position tick per whole second: the bar renders seconds, so a poll-rate tick in
/// between is a rebuild nobody sees.
struct SecondGate(u64);

impl Default for SecondGate {
    /// Past any real position, so a fresh track's first tick publishes.
    fn default() -> Self {
        Self(u64::MAX)
    }
}

impl SecondGate {
    /// A seek backwards admits too: what matters is that the second moved.
    fn admits(&mut self, position_ms: u64) -> bool {
        let second = position_ms / 1000;
        let moved = second != self.0;
        self.0 = second;
        moved
    }
}

/// The polls a pause has held an exclusive device for. It restarts once it answers, so a release
/// a seek turned down is asked again a period later rather than never.
#[derive(Default)]
struct PauseWatch(u64);

impl PauseWatch {
    /// Anything but a held pause restarts the count, so two pauses never add up.
    fn due(&mut self, holding: bool) -> bool {
        if !holding {
            self.0 = 0;
            return false;
        }
        self.0 += 1;
        if self.0 < RELEASE_AFTER_N_TICKS {
            return false;
        }
        self.0 = 0;
        true
    }
}

/// What the monitor read off the backend before the state lock, under which it would nest the
/// decks mutex.
#[derive(Copy, Clone)]
pub struct BackendSnapshot {
    /// The tick publishes the heard position; the crossfade and gapless stage time the pull.
    pub clock: DeckClock,
    pub already_preloaded: bool,
    pub crossfading: bool,
    pub xf: crossfade::CrossfadeSettings,
    /// The output has to reopen for the track queued next, which no crossfade can cross.
    pub next_needs_reopen: bool,
}

/// One `Playing` tick's decision: at most one of a crossfade or a gapless preload, two ways to
/// make the same transition.
pub struct PlayingTick {
    pub tick: PositionTick,
    /// The monitor keeps the record to seat once the deck hands over to it.
    pub late_preload: Option<Arc<TrackSummary>>,
    pub crossfade: Option<crossfade::CrossfadeDecision>,
}

/// A `Playing` tick's whole decision, pure so the crossfade-vs-gapless gate tests without a
/// backend. `None` where playback left `Playing` before the lock.
pub fn evaluate_playing_tick(
    state: &mut PlayerState,
    backend: BackendSnapshot,
) -> Option<PlayingTick> {
    let BackendSnapshot {
        clock: DeckClock { heard_ms: position_ms, pulled_ms },
        already_preloaded,
        crossfading,
        xf,
        next_needs_reopen,
    } = backend;

    if state.status != PlaybackStatus::Playing {
        return None;
    }
    state.position_ms = position_ms;
    state.stamp_position();
    let tick =
        PositionTick { position_ms, duration_ms: state.duration_ms, stamp: state.position_stamp };

    // A live source has no track end, which is all the two decisions below are about.
    if !state.source_allows(PlaybackSource::advances_queue) {
        return Some(PlayingTick { tick, late_preload: None, crossfade: None });
    }

    // A next track whose record the window doesn't hold yet is no next yet: neither the crossfade
    // nor the stage below can wait on a fetch, and the next poll asks again.
    let next = state.queue.peek_next().and_then(|id| state.record(id));
    let same_album = match (state.current_track(), next.as_ref()) {
        (Some(cur), Some(nxt)) => crossfade::same_album(cur, nxt),
        _ => false,
    };
    // Timing-independent, so the preload gated on its negation can't fire first for a crossfade
    // shorter than `PRELOAD_LEAD_MS` and block it. A next track needing a reopen reaches the
    // preload, which refuses it, so it ends at `EndOfStream` either way.
    let eligible = crossfade::crossfade_eligible(
        xf,
        state.pause_after_current_track || next_needs_reopen,
        next.is_some(),
        same_album,
    );

    // Carries what it was decided against, for `build_crossfade_actions` to re-verify.
    let crossfade = crossfade::should_crossfade(
        eligible,
        already_preloaded,
        crossfading,
        pulled_ms,
        state.duration_ms,
        xf.duration_ms,
    )
    .map(|fade_ms| crossfade::CrossfadeDecision {
        fade_ms,
        track_id: state.current_track().map(|t| t.id),
        position_ms: state.position_ms,
    });

    // Staged late, so a queue or repeat change mid-track still decides what plays next.
    let late_preload = if state.gapless_enabled
        && !already_preloaded
        && !eligible
        && !crossfading
        // The sleep timer's end-of-track mode needs an `EndOfStream` to catch.
        && !state.pause_after_current_track
        && state.duration_ms > 0
        // A deck reads 0 until pulled, and a track shorter than the lead would stage at once.
        && pulled_ms > 0
        && pulled_ms <= state.duration_ms
        && state.duration_ms.saturating_sub(pulled_ms) < PRELOAD_LEAD_MS
    {
        next
    } else {
        None
    };

    Some(PlayingTick { tick, late_preload, crossfade })
}

/// Asked only with crossfade on, since the first ask about a track opens its file (outside the
/// state lock), usually on the track's first tick.
fn next_needs_reopen(
    engine: &PlaybackEngine,
    player_state: &PlayerStateHandle,
    xf: crossfade::CrossfadeSettings,
) -> bool {
    if !xf.enabled {
        return false;
    }
    let next_path = {
        let state = lock_state(player_state);
        if !state.source_allows(PlaybackSource::advances_queue) {
            return false;
        }
        state.queue.peek_next().and_then(|id| state.record(id)).map(|track| track.file_path.clone())
    };
    next_path.is_some_and(|path| engine.reopens_for(&path))
}

/// Names the station that gave up, which is otherwise a silence with no explanation.
fn notify_station_ended(player_state: &PlayerStateHandle) {
    let station = lock_state(player_state).station().map(|s| s.name.clone());
    if let Some(name) = station {
        melodia_core::utils::toast::notify(
            melodia_core::utils::toast::ToastKind::PlaybackFailed,
            name,
        );
    }
}

/// Brings the state in line with the stream's buffering flag and ICY title, which change on the
/// feed thread. Polled on the monitor's tick, cheaper than a channel per station, and emitting
/// only on a change.
fn reconcile_live_stream(
    stream: &StreamShared,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    last_title_generation: &mut u64,
) {
    stream.publish_heard_title();
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
            // The one in-flight mutation, so the copy-on-write lands here, once a song.
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
    /// What a restart resumes, [`PlayerState::resume_point`].
    pub track: Option<(i64, u64)>,
    pub playback: PersistedPlayback,
}

/// A sink rather than a `DbPool`, where playback is written down being a question one layer up.
/// Awaited inline, so an in-flight save completes before shutdown wins the next select.
pub type SnapshotSink =
    Arc<dyn Fn(PlaybackSnapshot) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub struct PlaybackMonitorContext {
    pub shutdown_token: CancellationToken,
    pub player_state: Arc<PlayerStateHandle>,
    pub engine: Arc<PlaybackEngine>,
    pub sinks: Arc<PlayerSinks>,
    pub position_tx: watch::Sender<Option<PositionTick>>,
    pub signal_path_tx: watch::Sender<Option<SignalPath>>,
    pub save: SnapshotSink,
}

/// Spawns the task that polls the position and catches gapless handovers and stream ends.
pub fn spawn_playback_monitor(tracker: &TaskTracker, ctx: PlaybackMonitorContext) {
    let shutdown_token = ctx.shutdown_token.clone();
    let mut monitor = Monitor::new(ctx);
    tracker.spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
        loop {
            tokio::select! {
                biased;
                () = shutdown_token.cancelled() => {
                    log::info!("Playback monitor stopped");
                    break;
                }
                _ = interval.tick() => {}
            }
            if monitor.poll() == Poll::Counted {
                monitor.count_toward_save().await;
            }
        }
    });
}

/// Whether a poll counts toward the save cadence, which counts playback rather than wall time.
#[derive(PartialEq, Eq)]
enum Poll {
    Counted,
    Skipped,
}

/// The monitor's handles, and what it carries from one poll to the next.
struct Monitor {
    player_state: Arc<PlayerStateHandle>,
    engine: Arc<PlaybackEngine>,
    sinks: Arc<PlayerSinks>,
    position_tx: watch::Sender<Option<PositionTick>>,
    signal_path_tx: watch::Sender<Option<SignalPath>>,
    save: SnapshotSink,
    save_tick_counter: u64,
    publish: SecondGate,
    pause_watch: PauseWatch,
    /// The record of the track last staged behind the current one, which the deck plays once it
    /// hands over. Only the monitor stages, and a deck holds one stage at a time.
    staged: Option<Arc<TrackSummary>>,
    /// Last ICY title generation reconciled into `PlayerState`. Generations are process-wide
    /// tickets starting at 1, so this holds across stations and `0` means nothing seen yet.
    last_title_generation: u64,
}

impl Monitor {
    fn new(ctx: PlaybackMonitorContext) -> Self {
        Self {
            player_state: ctx.player_state,
            engine: ctx.engine,
            sinks: ctx.sinks,
            position_tx: ctx.position_tx,
            signal_path_tx: ctx.signal_path_tx,
            save: ctx.save,
            save_tick_counter: 0,
            publish: SecondGate::default(),
            pause_watch: PauseWatch::default(),
            staged: None,
            last_title_generation: 0,
        }
    }

    fn poll(&mut self) -> Poll {
        // Every read below takes the decks lock, which a reopen holds across a device open, and
        // waiting for it would block a runtime worker as long. The next tick reads the new stream.
        if self.engine.output_reopening() {
            return Poll::Skipped;
        }

        // Ahead of the not-playing short circuit below, because a stop is what retires the
        // most sources at once and it lands on exactly the ticks that circuit skips.
        self.engine.collect_spent();

        let status = self.player_state.status_atomic.load(std::sync::atomic::Ordering::Relaxed);
        let paused = status == PlaybackStatus::Paused as u8;
        let release_due = self.pause_watch.due(paused && self.engine.holds_releasable_claim());

        if status != PlaybackStatus::Playing as u8 {
            if release_due {
                release_paused_track(&self.engine, &self.player_state, &self.sinks);
            }
            // A pause keeps the last path: nothing pulls the source, so it could not be
            // re-read anyway. One whose device went back has none left to describe.
            if status == PlaybackStatus::Stopped as u8 || (paused && self.engine.output_parked()) {
                publish_signal_path(&self.signal_path_tx, None);
            }
            return Poll::Skipped;
        }

        match self.engine.check_playback_state() {
            PlaybackCheck::GaplessTransition => {
                self.advance_gapless();
                Poll::Counted
            }
            PlaybackCheck::EndOfStream => self.end_of_stream(),
            PlaybackCheck::Playing => self.playing(),
        }
    }

    fn advance_gapless(&mut self) {
        let staged = self.staged.take();
        let engine = &self.engine;
        emit_and_execute(&*self.engine, &self.player_state, &self.sinks, |state| {
            // Claimed under the emit: a start or stop since the check has cleared it, and
            // advancing anyway would step past the track the deck now plays.
            if !engine.claim_gapless_handover() {
                return Vec::new();
            }
            state.build_gapless_advance_actions(staged)
        });
    }

    fn end_of_stream(&self) -> Poll {
        // A station drains only once its reconnect budget is spent. An unfinished one is a deck
        // between `play_stream` publishing the cell and appending the source: not yet, not over.
        let live = self.engine.stream_shared();
        if live.as_deref().is_some_and(|s| !s.is_finished()) {
            return Poll::Skipped;
        }
        if live.is_some() {
            notify_station_ended(&self.player_state);
        }
        emit_and_execute(
            &*self.engine,
            &self.player_state,
            &self.sinks,
            PlayerState::build_end_of_stream_actions,
        );
        Poll::Counted
    }

    fn playing(&mut self) -> Poll {
        let xf = self.engine.crossfade_settings();
        // Ahead of the positions, which a first open of the next file would leave stale, and of
        // the exec lock, which the transport would otherwise wait on for that open.
        let next_needs_reopen = next_needs_reopen(&self.engine, &self.player_state, xf);
        let Some((decided, transport)) = self.read_and_publish(xf, next_needs_reopen) else {
            return Poll::Skipped;
        };
        publish_signal_path(&self.signal_path_tx, self.engine.signal_path(transport));
        let Some(PlayingTick { tick, late_preload, crossfade }) = decided else {
            return Poll::Skipped;
        };

        if let Some(mc) = self.sinks.media_controls.as_ref() {
            mc.update_position(tick.position_ms);
        }
        if let Some(stream) = self.engine.stream_shared() {
            reconcile_live_stream(
                &stream,
                &self.player_state,
                &self.sinks,
                &mut self.last_title_generation,
            );
        }

        if let Some(decision) = crossfade {
            emit_and_execute(&*self.engine, &self.player_state, &self.sinks, |state| {
                state.build_crossfade_actions(decision)
            });
        } else if let Some(track) = late_preload {
            // Its own tags' gain, baked into its own source.
            self.engine.preload_gapless(Some(&track.file_path), track.as_ref().into());
            self.staged = Some(track);
        }
        Poll::Counted
    }

    /// Reads the deck, decides and sends the tick in one exec-lock hold: a seek or track start
    /// moves the state before the deck, so a reading across one would publish the position it
    /// moved off. `None` while an op holds the lock or the deck lags one.
    fn read_and_publish(
        &mut self,
        xf: crossfade::CrossfadeSettings,
        next_needs_reopen: bool,
    ) -> Option<(Option<PlayingTick>, Transport)> {
        let _exec = self.player_state.try_lock_exec()?;
        let backend = self.read_backend(xf, next_needs_reopen)?;
        let (decided, transport) = {
            let mut state = lock_state(&self.player_state);
            let transport = Transport {
                volume: state.volume,
                muted: state.is_muted,
                speed: state.playback_speed,
                crossfading: backend.crossfading,
            };
            (evaluate_playing_tick(&mut state, backend), transport)
        };
        if let Some(PlayingTick { tick, .. }) = &decided
            && self.publish.admits(tick.position_ms)
        {
            let _ = self.position_tx.send(Some(tick.clone()));
        }
        Some((decided, transport))
    }

    /// `None` while the deck has yet to act on what it was sent.
    fn read_backend(
        &self,
        xf: crossfade::CrossfadeSettings,
        next_needs_reopen: bool,
    ) -> Option<BackendSnapshot> {
        Some(BackendSnapshot {
            clock: self.engine.query_settled_clock()?,
            already_preloaded: self.engine.is_gapless_preloaded(),
            crossfading: self.engine.is_crossfading(),
            xf,
            next_needs_reopen,
        })
    }

    async fn count_toward_save(&mut self) {
        self.save_tick_counter = (self.save_tick_counter + 1) % SAVE_EVERY_N_TICKS;
        if self.save_tick_counter != 0 {
            return;
        }
        let snapshot = {
            let state = lock_state(&self.player_state);
            PlaybackSnapshot { track: state.resume_point(), playback: state.to_persisted() }
        };
        (self.save)(snapshot).await;
    }
}

/// Gives a long-paused track's exclusive device back. The deck is read under the emit, and only
/// once settled: a seek the emit waited out may not have reached it, and its old position would
/// overwrite the seek's.
fn release_paused_track(
    engine: &PlaybackEngine,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
) {
    let seen = {
        let state = lock_state(player_state);
        state.current_track().map(|track| (track.id, state.position_ms))
    };
    let Some((track_id, position_ms)) = seen else {
        return;
    };
    emit_and_execute(engine, player_state, sinks, |state| {
        let Some(resume_ms) = engine.query_settled_position(position_ms) else {
            return Vec::new();
        };
        state.build_release_actions(ReleaseDecision { track_id, position_ms, resume_ms })
    });
}

/// Publish `path` only when it differs, so the panel repaints on a change rather than per tick.
fn publish_signal_path(tx: &watch::Sender<Option<SignalPath>>, path: Option<SignalPath>) {
    tx.send_if_modified(|current| {
        if *current == path {
            return false;
        }
        *current = path;
        true
    });
}

#[cfg(test)]
#[path = "tests/handlers_tests.rs"]
mod tests;
