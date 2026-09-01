use std::collections::HashMap;
use std::sync::Arc;
use std::sync::MutexGuard;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use super::crossfade::CrossfadeDecision;
use super::event_sink::PlayerSinks;
use super::queue::QueueState;
use super::replaygain::TrackReplayGain;
use crate::entities::track::TrackSummary;

use super::types::{
    PersistableQueue, PersistedPlayback, PlaybackSource, PlaybackStatus, RadioNowPlaying,
    RepeatMode,
};

/// Restart-from-beginning threshold for Previous command (ms).
pub const RESTART_THRESHOLD_MS: u64 = 3000;
/// Maximum volume level (percent) stored in `PlayerState` and reachable from the
/// UI. Playback amplitude tops out at unity gain (see [`volume_to_amplitude`]),
/// so this is the true ceiling — there is no boost band above it.
pub const MAX_VOLUME: u32 = 100;
/// Minimum playback speed multiplier.
pub const MIN_SPEED: f64 = 0.25;
/// Maximum playback speed multiplier. Capped at 2×: speed is a ratio on the
/// deck's converter, which is naive resampling and shifts pitch with it, so
/// beyond 2× the audio degrades into chipmunk territory with little practical
/// use for music.
pub const MAX_SPEED: f64 = 2.0;

/// Single source of truth for converting a stored volume level (percent,
/// `[0, MAX_VOLUME]`) plus a mute flag into the linear amplitude `[0.0, 1.0]`
/// the audio backend and OS media controls (MPRIS) both expect. Muted → 0.0.
/// `MAX_VOLUME` is the ceiling, so the result never exceeds unity gain.
pub fn volume_to_amplitude(volume: u32, is_muted: bool) -> f64 {
    if is_muted {
        0.0
    } else {
        f64::from(volume) / 100.0
    }
}

pub struct PlayerState {
    pub status: PlaybackStatus,
    /// What is on the deck, and what may be done with it — see [`PlaybackSource`].
    ///
    /// It was a track and a station held as separate `Option`s, mutually exclusive by an invariant
    /// only [`begin_track`] enforced, which left every reader inferring "not a track" from an
    /// absent one. Reach for [`Self::current_track`] and [`Self::station`] to read either half.
    pub source: Option<PlaybackSource>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: u32,
    pub is_muted: bool,
    pub pre_mute_volume: u32,
    pub playback_speed: f64,
    pub gapless_enabled: bool,
    /// Sleep-timer "End of current track" mode: when armed, the playback
    /// monitor pauses at the next end-of-stream boundary instead of advancing
    /// the queue. Session-only (never persisted). Set via
    /// [`crate::library::playback::player_set_pause_at_track_end`]; surfaced to
    /// the UI as `sleep_at_track_end` on the light `ViewModel` so the overflow
    /// menu's sleep row auto-clears once the monitor fires and disarms it.
    pub pause_after_current_track: bool,
    /// Which station session is current. Bumped by every transition that starts or ends one, so a
    /// connect that finishes after the user moved on is refused rather than played late. An open
    /// takes seconds and a click takes none, which is why this is a counter rather than a flag.
    pub radio_generation: u64,
    pub queue: QueueState,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            source: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 100,
            is_muted: false,
            pre_mute_volume: 100,
            playback_speed: 1.0,
            gapless_enabled: true,
            pause_after_current_track: false,
            radio_generation: 0,
            queue: QueueState::default(),
        }
    }
}

/// Wrapper around `Mutex<PlayerState>` with an atomic playback status mirror.
/// The `AtomicU8` allows the playback monitor to skip lock acquisition
/// when the player is paused/stopped (the common idle case).
pub struct PlayerStateHandle {
    mutex: std::sync::Mutex<PlayerState>,
    /// Mirror of `PlayerState::status` as a `u8`, updated after every state change.
    pub status_atomic: AtomicU8,
    /// Serializes the *side-effect* phase across tasks. `with_state_emit` makes a
    /// single state mutation atomic, but the `execute_actions` that follows runs
    /// on whatever tokio worker the caller happens to be on. Without this,
    /// two batches (e.g. the monitor's EOS-advance and a UI `Stop`) can interleave
    /// their side effects on separate workers and leave state and backend
    /// disagreeing. `emit_and_execute` holds this across *both* the mutation and
    /// the execution so mutation order equals side-effect order. Held only across
    /// synchronous work (never an `.await`), so a blocking mutex is correct.
    exec_lock: std::sync::Mutex<()>,
}

impl Default for PlayerStateHandle {
    fn default() -> Self {
        Self {
            mutex: std::sync::Mutex::new(PlayerState::default()),
            status_atomic: AtomicU8::new(PlaybackStatus::Stopped as u8),
            exec_lock: std::sync::Mutex::new(()),
        }
    }
}

impl PlayerStateHandle {
    /// Acquire the execution lock, recovering from poison rather than panicking
    /// (mirrors [`lock_state`] / `PlaybackEngine::lock_decks`). The guarded unit
    /// carries no data — poison only means a prior holder panicked mid-batch, and
    /// the guard exists purely to serialize the next batch.
    pub fn lock_exec(&self) -> MutexGuard<'_, ()> {
        self.exec_lock.lock().unwrap_or_else(|poisoned| {
            log::error!("PlayerState exec lock was poisoned, recovering");
            poisoned.into_inner()
        })
    }
}

/// Lightweight event for 500ms position ticks — avoids serializing the full queue.
#[derive(Debug, Clone, Serialize)]
pub struct PositionTick {
    pub position_ms: u64,
    pub duration_ms: u64,
}

/// Full `ViewModel` — kept for tests only. Production publishes via
/// `PlayerViewModelLight` (on every state change) + `QueueViewModel` (on
/// queue-version changes) through separate watch channels so subscribers
/// don't re-clone the queue projection on every player tick.
///
/// The bool fields mirror the Slint `PlayerVm`/`QueueVm` structs in
/// `melodia-ui/ui/models.slint`; the shape must match exactly across the boundary, so
/// they cannot be collapsed into a bitflags wrapper.
#[allow(
    clippy::struct_excessive_bools,
    reason = "ViewModel mirrors Slint struct shape across the FFI boundary"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerViewModel {
    pub status: String,
    pub current_track: Option<Arc<TrackSummary>>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub progress_percent: f64,
    pub volume: u32,
    pub is_muted: bool,
    pub playback_speed: f64,
    pub gapless_enabled: bool,
    pub sleep_at_track_end: bool,
    pub radio: Option<Arc<RadioNowPlaying>>,
    pub queue_tracks: Vec<Arc<TrackSummary>>,
    pub queue_index: i32,
    pub shuffle_enabled: bool,
    pub repeat_mode: RepeatMode,
    pub has_next: bool,
    pub has_previous: bool,
}

/// Lightweight `ViewModel` emitted on every state change — excludes queue data.
#[allow(
    clippy::struct_excessive_bools,
    reason = "ViewModel mirrors Slint struct shape across the FFI boundary"
)]
#[derive(Debug, Clone, Serialize)]
pub struct PlayerViewModelLight {
    pub status: &'static str,
    pub current_track: Option<Arc<TrackSummary>>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub progress_percent: f64,
    pub volume: u32,
    pub is_muted: bool,
    pub playback_speed: f64,
    pub gapless_enabled: bool,
    pub sleep_at_track_end: bool,
    /// The station playing, when the source is a live one. `current_track` is `None` throughout,
    /// so a surface reads whichever of the two is `Some` rather than a flag saying which to trust.
    /// Its `buffering` is where the spinner comes from.
    pub radio: Option<Arc<RadioNowPlaying>>,
    pub has_next: bool,
    pub has_previous: bool,
}

/// Queue-specific `ViewModel` emitted only when the queue changes.
#[derive(Debug, Clone, Serialize)]
pub struct QueueViewModel {
    pub queue_tracks: Vec<Arc<TrackSummary>>,
    pub queue_index: i32,
    pub shuffle_enabled: bool,
    pub repeat_mode: RepeatMode,
    pub has_next: bool,
    pub has_previous: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerAction {
    PlayMedia {
        file_path: String,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        /// This track's baked `ReplayGain` tag values, applied by the audio source.
        replaygain: TrackReplayGain,
    },
    /// Overlap the next track with the one still playing, fading between them
    /// over `fade_ms` **media** milliseconds. Unlike `PlayMedia` this leaves the
    /// current track audible; the backend runs the two on separate decks.
    BeginCrossfade {
        file_path: String,
        /// The *incoming* track's baked `ReplayGain` values. Baked per source —
        /// the outgoing track has its own, already applied.
        replaygain: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    },
    Resume,
    /// `fade_ms` is the pause-fade length for a user-initiated pause, and `0`
    /// where a fade would be wrong: next / previous pressed *while paused* emit
    /// `PlayMedia` (which starts the deck) followed by this, purely to restore
    /// the paused state — fading there would let the incoming track play its
    /// first `fade_ms` out loud on arrival.
    Pause {
        fade_ms: u64,
    },
    /// `fade_ms` is `0` for an internal stop (end of queue, error recovery) and
    /// the pause-fade length for a user-initiated stop.
    Stop {
        fade_ms: u64,
    },
    /// Move the playing track's position.
    ///
    /// Carries the file and its gain for the same reason [`Self::PlayMedia`] does: the backend
    /// seeks by building a source already at the target, so a seek rebuilds what the deck holds
    /// and the new source needs its own baked values. Only ever emitted for a track, a live
    /// source having no timeline to land on.
    Seek {
        position_ms: u64,
        file_path: String,
        replaygain: TrackReplayGain,
    },
    SetVolume(f64),
    SetSpeed(f64),
    PreloadGapless(Option<String>),
    /// Start the live stream the backend already has staged for this station session.
    ///
    /// Carries neither a URL nor a reader: opening a stream is a network round trip and belongs on
    /// the async task that started the station, not on the executor thread. `generation` is what
    /// the backend checks the stage against, so an action that outlived its session plays nothing.
    PlayStream {
        generation: u64,
        volume: f64,
    },
    UpdatePlayCount(i64),
    UpdateSkipCount(i64),
}

/// The verbose log's playback narrative, one action per line.
///
/// Terse where the derived `Debug` is exhaustive — `PlayMedia` alone would print
/// a whole `TrackReplayGain`. On the enum rather than in `execute_actions` so a
/// new variant is a non-exhaustive-match failure here, where a `_ =>` arm in the
/// executor would have logged it as nothing at all.
impl std::fmt::Display for PlayerAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlayMedia {
                file_path,
                start_position_ms,
                ..
            } => match start_position_ms {
                Some(ms) => write!(f, "play {file_path} from {ms}ms"),
                None => write!(f, "play {file_path}"),
            },
            Self::BeginCrossfade {
                file_path, fade_ms, ..
            } => {
                write!(f, "crossfade {fade_ms}ms into {file_path}")
            }
            Self::Resume => f.write_str("resume"),
            Self::Pause { fade_ms } => write!(f, "pause (fade {fade_ms}ms)"),
            Self::Stop { fade_ms } => write!(f, "stop (fade {fade_ms}ms)"),
            Self::Seek { position_ms, .. } => write!(f, "seek to {position_ms}ms"),
            Self::SetVolume(v) => write!(f, "volume {v:.2}"),
            Self::SetSpeed(s) => write!(f, "speed {s:.2}"),
            Self::PreloadGapless(Some(path)) => write!(f, "preload gapless {path}"),
            Self::PreloadGapless(None) => f.write_str("clear gapless preload"),
            // No URL: a station's stream URL can carry a session token, and this line goes into
            // the log tail users attach to public issues.
            Self::PlayStream { generation, .. } => write!(f, "play radio stream #{generation}"),
            Self::UpdatePlayCount(id) => write!(f, "play count +1 for track {id}"),
            Self::UpdateSkipCount(id) => write!(f, "skip count +1 for track {id}"),
        }
    }
}

impl PlayerState {
    fn progress_percent(&self) -> f64 {
        if self.duration_ms > 0 {
            (ms_to_f64(self.position_ms) / ms_to_f64(self.duration_ms)) * 100.0
        } else {
            0.0
        }
    }

    /// The track on the deck, or `None` when the source is a live one or there is none.
    pub fn current_track(&self) -> Option<&Arc<TrackSummary>> {
        self.source.as_ref().and_then(PlaybackSource::track)
    }

    /// The track on the deck, for the two writers that mutate it in place.
    pub fn current_track_mut(&mut self) -> Option<&mut Arc<TrackSummary>> {
        self.source.as_mut().and_then(PlaybackSource::track_mut)
    }

    /// The station on the deck, including through the stretch where it is still connecting.
    pub fn station(&self) -> Option<&Arc<RadioNowPlaying>> {
        self.source.as_ref().and_then(PlaybackSource::station)
    }

    /// The station on the deck, for the live title and the buffering flag.
    pub fn station_mut(&mut self) -> Option<&mut Arc<RadioNowPlaying>> {
        self.source.as_mut().and_then(PlaybackSource::station_mut)
    }

    /// Whether the source on the deck is a live stream.
    ///
    /// Five things ask this rather than asking a capability, and every one of them is *about* a
    /// station: the pause that drops its socket, the play that cannot resume from one, the stop
    /// that forgets it, the session check, and the track that evicts it. Everywhere else the
    /// question is what the source can do, not what it is, and asking it that way is what stops a
    /// third kind of source being silently treated as a file.
    fn is_radio(&self) -> bool {
        self.station().is_some()
    }

    /// Whether what is on the deck permits `capability`.
    ///
    /// **An empty deck permits everything**, which is why this is `is_none_or` and not
    /// `is_some_and`. Each caller is asking whether what is playing rules the action out, not
    /// whether something is playing: a loaded queue with nothing on the deck still offers Next,
    /// and gating that on a source being present silently disables it.
    ///
    /// Takes the capability rather than exposing one accessor per question, so a third kind of
    /// source is a match arm on [`PlaybackSource`] and nothing here.
    pub fn source_allows(&self, capability: fn(&PlaybackSource) -> bool) -> bool {
        self.source.as_ref().is_none_or(capability)
    }

    fn has_next(&self) -> bool {
        self.source_allows(PlaybackSource::advances_queue) && self.queue.peek_next().is_some()
    }

    fn has_previous(&self) -> bool {
        self.source_allows(PlaybackSource::advances_queue)
            && !self.queue.play_order.is_empty()
            && (self.queue.current_index.is_some_and(|ci| ci > 0) || self.queue.repeat_mode.wraps())
    }

    /// Full `ViewModel` — kept only for tests that assert against the
    /// composed state. Production traffic flows through `to_view_model_light`
    /// plus `to_queue_view_model` so the queue projection is rebuilt only
    /// when the queue actually changes, not on every player-state emit.
    #[cfg(test)]
    pub fn to_view_model(&self) -> PlayerViewModel {
        PlayerViewModel {
            status: self.status.as_str().to_owned(),
            current_track: self.current_track().cloned(),
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            progress_percent: self.progress_percent(),
            volume: self.volume,
            is_muted: self.is_muted,
            playback_speed: self.playback_speed,
            gapless_enabled: self.gapless_enabled,
            sleep_at_track_end: self.pause_after_current_track,
            radio: self.station().cloned(),
            queue_tracks: self.queue.tracks_in_play_order(),
            queue_index: super::queue::current_index_to_i32(self.queue.current_index),
            shuffle_enabled: self.queue.shuffle_enabled,
            repeat_mode: self.queue.repeat_mode,
            has_next: self.has_next(),
            has_previous: self.has_previous(),
        }
    }

    /// Lightweight `ViewModel` — excludes queue data for smaller payloads.
    pub fn to_view_model_light(&self) -> PlayerViewModelLight {
        PlayerViewModelLight {
            status: self.status.as_str(),
            current_track: self.current_track().cloned(),
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            progress_percent: self.progress_percent(),
            volume: self.volume,
            is_muted: self.is_muted,
            playback_speed: self.playback_speed,
            gapless_enabled: self.gapless_enabled,
            sleep_at_track_end: self.pause_after_current_track,
            radio: self.station().cloned(),
            has_next: self.has_next(),
            has_previous: self.has_previous(),
        }
    }

    /// Queue-only `ViewModel` — emitted only when the queue changes.
    pub fn to_queue_view_model(&self) -> QueueViewModel {
        QueueViewModel {
            queue_tracks: self.queue.tracks_in_play_order(),
            queue_index: super::queue::current_index_to_i32(self.queue.current_index),
            shuffle_enabled: self.queue.shuffle_enabled,
            repeat_mode: self.queue.repeat_mode,
            has_next: self.has_next(),
            has_previous: self.has_previous(),
        }
    }

    /// The snapshot `queue.json` holds, for the two save sites to write.
    ///
    /// Takes the whole state rather than the queue alone because a station rides beside the
    /// queue rather than in it, and both come back together at boot.
    pub fn to_persisted(&self) -> PersistedPlayback {
        PersistedPlayback {
            queue: self.queue.to_persistable(),
            // A station with no row cannot be looked back up, so there is nothing to write down.
            station_id: self.station().map(|s| s.station_id).filter(|id| *id != 0),
        }
    }

    /// Convert this state's volume for the audio backend: stored `[0, MAX_VOLUME]`,
    /// backend gets `[0.0, 1.0]`. Returns 0.0 when muted. Thin wrapper over the
    /// shared [`volume_to_amplitude`] so the MPRIS path can reuse the same math.
    pub fn effective_volume(&self) -> f64 {
        volume_to_amplitude(self.volume, self.is_muted)
    }

    /// Build actions for play/resume command.
    ///
    /// A paused station has no deck contents to resume — pausing dropped its connection — and
    /// re-opening one is a network round trip that cannot happen under the state lock. So this
    /// declines, and `library::playback::player_play` takes the station branch ahead of it.
    pub fn build_play_actions(&mut self) -> Vec<PlayerAction> {
        if self.is_radio() {
            return vec![];
        }
        if self.status == PlaybackStatus::Paused {
            self.status = PlaybackStatus::Playing;
            vec![PlayerAction::Resume]
        } else {
            resume_from_stopped(self)
        }
    }

    /// Build actions for pause command. `fade_ms` is the pause-fade length when
    /// that setting is on, else `0` — same contract as [`Self::build_stop_actions`].
    ///
    /// Pausing a station **drops its connection**: `stream-download` pauses its writer when the
    /// reader falls behind, so a held-open socket would back-pressure the server and come back
    /// playing audio that is seconds stale. The station stays on screen with a play button that
    /// re-opens it, which is what Shortwave and `RadioDroid` do.
    pub fn build_pause_actions(&mut self, fade_ms: u64) -> Vec<PlayerAction> {
        if self.is_radio() && self.status != PlaybackStatus::Stopped {
            self.status = PlaybackStatus::Paused;
            self.end_stream_session();
            return vec![PlayerAction::Stop { fade_ms }];
        }
        if self.status == PlaybackStatus::Playing {
            self.status = PlaybackStatus::Paused;
            vec![PlayerAction::Pause { fade_ms }]
        } else {
            vec![]
        }
    }

    /// Build actions for user-initiated stop (preserves position for resume).
    /// `fade_ms` is the pause-fade length when that setting is on, else `0`.
    ///
    /// A station is forgotten outright rather than paused, so the transport falls back to the
    /// queue that was left untouched underneath it (D9).
    pub fn build_stop_actions(&mut self, fade_ms: u64) -> Vec<PlayerAction> {
        self.status = PlaybackStatus::Stopped;
        if self.is_radio() {
            self.end_stream_session();
            self.source = None;
        }
        vec![PlayerAction::Stop { fade_ms }]
    }

    /// The seek action for whatever track is on the deck, or nothing where none is.
    ///
    /// The track rides along because the backend rebuilds the source to move it; see
    /// [`PlayerAction::Seek`].
    fn seek_action(&self, position_ms: u64) -> Option<PlayerAction> {
        let track = self.source.as_ref().and_then(PlaybackSource::track)?;
        Some(PlayerAction::Seek {
            position_ms,
            file_path: track.file_path.clone(),
            replaygain: track.replaygain(),
        })
    }

    /// Build actions for seek command.
    pub fn build_seek_actions(&mut self, position_ms: u64) -> Vec<PlayerAction> {
        // A live stream has no timeline to land on; the position is elapsed listening time.
        if !self.source_allows(PlaybackSource::is_seekable) {
            return vec![];
        }
        // The position moves either way. Nothing seated is nothing for the backend to rebuild, but
        // `source_allows` passes on an absent source, so this is reachable before anything has
        // played — where the action the old in-place seek emitted was a no-op by the time it
        // landed on an empty deck.
        let seek = self.seek_action(position_ms);
        self.position_ms = position_ms;
        seek.into_iter().collect()
    }

    /// Build actions for next-track command.
    pub fn build_next_actions(&mut self) -> Vec<PlayerAction> {
        if !self.source_allows(PlaybackSource::advances_queue) {
            return vec![];
        }
        let mut actions = vec![];
        let was_paused = self.status == PlaybackStatus::Paused;

        if let Some(track) = self.current_track()
            && self.duration_ms > 0
            && self.position_ms < self.duration_ms / 2
        {
            actions.push(PlayerAction::UpdateSkipCount(track.id));
        }

        if let Some(track) = self.queue.advance_skip().cloned() {
            actions.extend(play_track_inner(self, track, None));
            self.restore_paused(was_paused, &mut actions);
        } else {
            actions.extend(stop_end_of_queue(self));
        }

        actions
    }

    /// Re-pause after a track change that was made while paused.
    ///
    /// `fade_ms: 0` is load-bearing: the `PlayMedia` this follows has just
    /// *started* the deck, and this only restores the paused state. A fade here
    /// would ramp the incoming track down from full volume instead of pausing it,
    /// so its first quarter-second would be audible — and its decoder would be
    /// that far in on resume.
    fn restore_paused(&mut self, was_paused: bool, actions: &mut Vec<PlayerAction>) {
        if was_paused {
            self.status = PlaybackStatus::Paused;
            actions.push(PlayerAction::Pause { fade_ms: 0 });
        }
    }

    /// Build actions for previous-track command.
    pub fn build_previous_actions(&mut self) -> Vec<PlayerAction> {
        if !self.source_allows(PlaybackSource::advances_queue) {
            return vec![];
        }
        let was_paused = self.status == PlaybackStatus::Paused;

        if self.position_ms > RESTART_THRESHOLD_MS {
            let restart = self.seek_action(0);
            self.position_ms = 0;
            return restart.into_iter().collect();
        }

        if let Some(track) = self.queue.previous().cloned() {
            let mut actions = play_track_inner(self, track, None);
            self.restore_paused(was_paused, &mut actions);
            actions
        } else {
            let restart = self.seek_action(0);
            self.position_ms = 0;
            restart.into_iter().collect()
        }
    }

    /// Build actions for set-volume command.
    pub fn build_set_volume_actions(&mut self, level: u32) -> Vec<PlayerAction> {
        self.volume = level.min(MAX_VOLUME);
        self.is_muted = false;
        vec![PlayerAction::SetVolume(self.effective_volume())]
    }

    /// Build actions for set-muted command.
    pub fn build_set_muted_actions(&mut self, muted: bool) -> Vec<PlayerAction> {
        if muted && !self.is_muted {
            self.pre_mute_volume = self.volume;
        }
        self.is_muted = muted;
        vec![PlayerAction::SetVolume(self.effective_volume())]
    }

    /// Build actions for toggle-mute command.
    pub fn build_toggle_mute_actions(&mut self) -> Vec<PlayerAction> {
        let new_muted = !self.is_muted;
        if new_muted {
            self.pre_mute_volume = self.volume;
        }
        self.is_muted = new_muted;
        vec![PlayerAction::SetVolume(self.effective_volume())]
    }

    /// Build actions when the current source drains to end-of-stream (the
    /// playback monitor's `EndOfStream` branch). Normally advances the queue
    /// (or stops at the end), but when the sleep-timer's "End of current track"
    /// mode is armed it disarms the flag and stops instead of advancing —
    /// leaving `current_track` at position 0 for replay-from-start. Always
    /// counts a play for the track that just ended.
    pub fn build_end_of_stream_actions(&mut self) -> Vec<PlayerAction> {
        // A station's deck only drains once its feed thread has spent its reconnect budget, so
        // there is nothing to advance to — the queue underneath belongs to the library, and
        // wandering into it because a station went off air would be a silent change of source.
        if !self.source_allows(PlaybackSource::advances_queue) {
            return self.build_stop_actions(0);
        }

        let mut actions = Vec::with_capacity(4);

        if let Some(track) = self.current_track() {
            actions.push(PlayerAction::UpdatePlayCount(track.id));
        }

        if self.pause_after_current_track {
            self.pause_after_current_track = false;
            actions.extend(stop_end_of_queue(self));
            return actions;
        }

        if let Some(track) = self.queue.advance().cloned() {
            actions.extend(play_track_inner(self, track, None));
        } else {
            actions.extend(stop_end_of_queue(self));
        }

        actions
    }

    /// Build actions when the playback monitor decides the current track should
    /// start overlapping the next one. Mirrors `build_end_of_stream_actions`,
    /// but the outgoing track stays audible for `fade_ms` while the incoming
    /// one ramps up on the other deck.
    ///
    /// State advances at fade *start*, so Now-Playing switches to the incoming
    /// track as the overlap begins — the behaviour Strawberry and mpd have.
    /// Returns an empty vec (no crossfade) when the queue has moved on and
    /// there is no longer a next track, or when the decision has gone stale.
    ///
    /// `decision` carries the state the monitor was looking at when it chose to
    /// crossfade. It makes that choice under the `PlayerState` lock but only
    /// reaches here after acquiring `exec_lock`, so any other control op — pause,
    /// stop, next, previous, picking a track, seeking — can complete in between.
    /// Re-verifying here is the same discipline as the `queue.advance()` below:
    ///
    /// - **status** — without it, forcing `Playing` would resurrect playback the
    ///   user just paused. `BeginCrossfade` calls `Player::play()`, so it really
    ///   would be audible.
    /// - **track id** — without it, `advance()` would skip straight past the
    ///   track they just picked.
    /// - **position** — the one the other two miss. A seek keeps both the status
    ///   and the id and moves only the position, so a backward scrub inside the
    ///   fade window would otherwise fade out and skip the track the user just
    ///   scrubbed *into*. The monitor writes `position_ms` itself immediately
    ///   before deciding, so in this window the only other writers are
    ///   [`build_seek_actions`](Self::build_seek_actions), [`play_track_inner`]
    ///   and [`build_previous_actions`](Self::build_previous_actions) — exactly
    ///   the ops that must abort. Equality therefore also covers the *same* track
    ///   being restarted (which resets the position to 0).
    pub fn build_crossfade_actions(&mut self, decision: CrossfadeDecision) -> Vec<PlayerAction> {
        let mut actions = Vec::with_capacity(2);

        if !self.source_allows(PlaybackSource::advances_queue) {
            return actions;
        }

        let Some(outgoing_id) = self.current_track().map(|t| t.id) else {
            return actions;
        };
        if self.status != PlaybackStatus::Playing
            || Some(outgoing_id) != decision.track_id
            || self.position_ms != decision.position_ms
        {
            return actions;
        }

        // Re-read the queue under the emit lock rather than trusting the
        // monitor's earlier `peek_next` — a skip could have landed in between.
        let Some(track) = self.queue.advance().cloned() else {
            return actions;
        };

        // The outgoing track counts as played the moment it starts fading. Same
        // accounting as `build_end_of_stream_actions`, just a few seconds early —
        // and only once `advance()` has confirmed somewhere to go.
        actions.push(PlayerAction::UpdatePlayCount(outgoing_id));

        // Same "the state now points at this track" step `play_track_inner`
        // takes — only the action it ends in differs. (Its `status = Playing` is
        // a no-op here; the guard above already proved it.)
        let start = begin_track(self, track, None);

        actions.push(PlayerAction::BeginCrossfade {
            file_path: start.file_path,
            replaygain: start.replaygain,
            fade_ms: decision.fade_ms,
            volume: start.volume,
            speed: start.speed,
        });
        actions
    }

    /// Build actions for set-playback-speed command.
    pub fn build_set_speed_actions(&mut self, speed: f64) -> Vec<PlayerAction> {
        // Speed is a ratio on the deck's converter, so anything but 1.0 consumes a source faster or
        // slower than real time, and a live mount arriving at exactly real time starves or overruns
        // its ring. Refused rather than clamped, so the transport keeps showing the 1.0 the deck is
        // actually running at.
        if !self.source_allows(PlaybackSource::has_variable_speed) {
            return vec![];
        }
        let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        self.playback_speed = speed;
        vec![PlayerAction::SetSpeed(speed)]
    }

    /// Point the state at `station` and start connecting to it.
    ///
    /// Returns the session generation the caller must carry through to
    /// [`Self::build_station_connected_actions`], and the actions that clear the decks: the queue
    /// itself is deliberately untouched, so stopping the station later resumes the library exactly
    /// where it was (D9).
    ///
    /// Speed is reset alongside, because [`Self::build_set_speed_actions`] refuses to move it
    /// while a station plays and the transport would otherwise claim a rate the deck is not
    /// running at. The `SetSpeed` follows the `Stop` so it lands on emptied decks.
    pub fn build_station_connecting_actions(
        &mut self,
        station: Arc<RadioNowPlaying>,
    ) -> (u64, Vec<PlayerAction>) {
        self.end_stream_session();
        self.status = PlaybackStatus::Loading;
        self.source = Some(PlaybackSource::Station(station));
        self.duration_ms = 0;
        self.position_ms = 0;
        self.pause_after_current_track = false;

        let mut actions = vec![PlayerAction::Stop { fade_ms: 0 }];
        actions.extend(self.pin_speed_for_station());
        (self.radio_generation, actions)
    }

    /// Put the deck back on 1.0 for a station that is about to sit on it, and hand back the
    /// action that lands it on the deck.
    ///
    /// Shared with [`restore_station`], the one other way a station reaches the deck, because the
    /// state and the backend have to move together: skip the action and a stream opens against a
    /// deck still resampling, skip the field and the disabled speed row keeps quoting a rate
    /// nothing runs at.
    fn pin_speed_for_station(&mut self) -> Option<PlayerAction> {
        if (self.playback_speed - 1.0).abs() <= f64::EPSILON {
            return None;
        }
        self.playback_speed = 1.0;
        Some(PlayerAction::SetSpeed(1.0))
    }

    /// The stream opened: start it, unless the user moved on while it was connecting.
    pub fn build_station_connected_actions(&mut self, generation: u64) -> Vec<PlayerAction> {
        if !self.is_current_station_session(generation) {
            return vec![];
        }
        self.status = PlaybackStatus::Playing;
        vec![PlayerAction::PlayStream {
            generation,
            volume: self.effective_volume(),
        }]
    }

    /// The stream could not be opened. Clears the station rather than leaving a play button that
    /// would only fail the same way; the caller has already said so in a toast.
    pub fn build_station_failed_actions(&mut self, generation: u64) -> Vec<PlayerAction> {
        if !self.is_current_station_session(generation) {
            return vec![];
        }
        self.source = None;
        self.status = PlaybackStatus::Stopped;
        vec![PlayerAction::Stop { fade_ms: 0 }]
    }

    /// Whether `generation` is still the session the state is on. An open takes seconds and a
    /// click takes none, so anything arriving from one has to ask.
    fn is_current_station_session(&self, generation: u64) -> bool {
        self.is_radio() && self.radio_generation == generation
    }

    /// Invalidate the current station session, so whatever is still connecting for it is refused
    /// rather than played late. Called by every transition that starts or ends one.
    fn end_stream_session(&mut self) {
        self.radio_generation = self.radio_generation.wrapping_add(1);
        if let Some(station) = self.station_mut() {
            Arc::make_mut(station).buffering = false;
        }
    }
}

/// Everything a start action needs about the track the state now points at.
/// Produced by [`begin_track`], which is the single writer of the
/// "`current_track` + duration + position" trio.
struct TrackStart {
    file_path: String,
    replaygain: TrackReplayGain,
    volume: f64,
    speed: f64,
    /// The resume position, clamped and normalised — `None` means "from the top".
    start_position_ms: Option<u64>,
}

/// Point `state` at `track`: status Playing, duration and position from the
/// track, `current_track` replaced. Shared by [`play_track_inner`] (which turns
/// it into a `PlayMedia`) and [`PlayerState::build_crossfade_actions`] (a
/// `BeginCrossfade`), so the two can't drift on what "now playing this" means.
fn begin_track(
    state: &mut PlayerState,
    track: Arc<TrackSummary>,
    start_position_ms: Option<u64>,
) -> TrackStart {
    // Seating the track below evicts a station on its own, the two being one field now. What
    // still has to be said out loud is the session: a connect in flight would otherwise pass its
    // generation check and start the station over the track that replaced it.
    if state.is_radio() {
        state.end_stream_session();
    }

    state.status = PlaybackStatus::Playing;
    state.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
    // Clamp to 500ms before end to avoid immediate EOS detection by the playback monitor.
    let max_resume_pos = state.duration_ms.saturating_sub(500);
    let clamped_pos = start_position_ms.map(|p| p.min(max_resume_pos)).filter(|&p| p > 0);
    state.position_ms = clamped_pos.unwrap_or(0);

    let start = TrackStart {
        file_path: track.file_path.clone(),
        replaygain: track.replaygain(),
        volume: state.effective_volume(),
        speed: state.playback_speed,
        start_position_ms: clamped_pos,
    };
    state.source = Some(PlaybackSource::Track(track));
    start
}

/// Core playback logic — reused by commands, bus handler, position poller.
/// Returns `Vec<PlayerAction>` for the caller to execute after releasing the state lock.
/// `start_position_ms` — if `Some`, seeks to that position after starting playback (used for resume).
pub fn play_track_inner(
    state: &mut PlayerState,
    track: Arc<TrackSummary>,
    start_position_ms: Option<u64>,
) -> Vec<PlayerAction> {
    let start = begin_track(state, track, start_position_ms);

    // Gapless preload is staged late (by the playback monitor) when the
    // current track approaches its end — see `spawn_playback_monitor`. That
    // way mid-track repeat-mode / queue changes are reflected in what gets
    // preloaded, instead of being clobbered by a source already staged on the deck.
    vec![PlayerAction::PlayMedia {
        file_path: start.file_path,
        volume: start.volume,
        speed: start.speed,
        start_position_ms: start.start_position_ms,
        replaygain: start.replaygain,
    }]
}

/// Stop at end of queue — preserves `current_track` but resets position to 0 for replay-from-start.
/// Contrast with `player_stop` command which preserves position for resume-from-where-stopped.
pub fn stop_end_of_queue(state: &mut PlayerState) -> Vec<PlayerAction> {
    state.status = PlaybackStatus::Stopped;
    state.position_ms = 0;
    // Never faded: the track has already run out of audio, so there is nothing
    // left to fade — and a deferred clear would only delay the silence.
    vec![PlayerAction::Stop { fade_ms: 0 }]
}

/// Resume playback from a Stopped state. Replays the current track from the saved position.
/// Returns empty vec if status is not Stopped, or if neither the deck nor the queue holds a track.
///
/// **The queue is the fallback because stopping a station leaves no `current_track`** — a station
/// clears it on the way in and `build_stop_actions` forgets the station rather than pausing it, so
/// without this the play button is inert over a queue that is still fully seated. That is the
/// library D9 promised to hand back, and there is no position to resume from: the station zeroed it.
pub fn resume_from_stopped(state: &mut PlayerState) -> Vec<PlayerAction> {
    if state.status != PlaybackStatus::Stopped {
        return vec![];
    }
    if let Some(track) = state.current_track().cloned() {
        let resume_pos = (state.position_ms > 0).then_some(state.position_ms);
        return play_track_inner(state, track, resume_pos);
    }
    match state.queue.get_current().cloned() {
        Some(track) => play_track_inner(state, track, None),
        None => vec![],
    }
}

/// Lock the player state, recovering from mutex poison rather than panicking.
/// A poisoned mutex means a thread panicked while holding the lock — the state
/// may be inconsistent, but crashing the entire app is worse for a media player.
pub fn lock_state(handle: &PlayerStateHandle) -> MutexGuard<'_, PlayerState> {
    handle.mutex.lock().unwrap_or_else(|poisoned| {
        log::error!("PlayerState mutex was poisoned, recovering");
        poisoned.into_inner()
    })
}

/// Lock state, run mutation, build `ViewModel`, publish on the watch channels.
/// Makes forgetting `ViewModel` emission structurally impossible.
///
/// Always publishes a lightweight `ViewModel` (no queue data) on `sinks.view_model`.
/// Publishes a queue `ViewModel` on `sinks.queue` only when the queue version changed.
/// Synchronizes OS media controls (MPRIS / SMTC) when a sink is registered.
pub fn with_state_emit<F, R>(state: &PlayerStateHandle, sinks: &PlayerSinks, f: F) -> R
where
    F: FnOnce(&mut PlayerState) -> R,
{
    let mut guard = lock_state(state);
    let queue_version_before = guard.queue.version;
    let result = f(&mut guard);
    let status = guard.status;
    // Sync atomic mirror for lock-free status checks (e.g., playback monitor)
    state.status_atomic.store(status as u8, Ordering::Relaxed);
    let vm_light = guard.to_view_model_light();
    let queue_vm = if guard.queue.version == queue_version_before {
        None
    } else {
        Some(guard.to_queue_view_model())
    };
    drop(guard);

    // Order matters: `mc.sync` borrows `vm_light`, so we run it first and
    // then move `vm_light` into the watch-channel send by value. Reversed,
    // the send would have to clone.
    if let Some(qvm) = queue_vm {
        let _ = sinks.queue.send(Some(qvm));
    }
    if let Some(mc) = sinks.media_controls.as_ref() {
        mc.sync(&vm_light, status);
    }
    let _ = sinks.view_model.send(Some(vm_light));

    result
}

/// If `current_track` is one of `ids`, apply `apply` to its cached
/// [`TrackSummary`] and emit so the Now-Playing surfaces reflect a per-track
/// field edited from a list row (favorite heart, rating stars). Skips the emit
/// entirely when the playing track isn't in the set (the common case) to avoid
/// a spurious view-model publish.
pub fn sync_current_track_if_in(
    state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    ids: &[i64],
    apply: impl FnOnce(&mut TrackSummary),
) {
    let affects_current = {
        let g = lock_state(state);
        g.current_track().is_some_and(|t| ids.contains(&t.id))
    };
    if !affects_current {
        return;
    }
    with_state_emit(state, sinks, |s| {
        // Re-check the id under the emit lock — the track may have advanced
        // between the pre-check and here.
        if let Some(track) = s.current_track_mut()
            && ids.contains(&track.id)
        {
            apply(Arc::make_mut(track));
        }
        Vec::<PlayerAction>::new()
    });
}

/// True when `current_track` or any `queue.tracks` entry has an id the
/// predicate accepts. Takes the state lock briefly and reads nothing else, so a
/// caller can cheaply decide whether a resync (and the DB refetch that feeds
/// it) is worth doing before paying for it — the membership gate
/// [`sync_track_summaries`] uses internally, hoisted so the fetch itself can be
/// skipped on the common "edited tracks aren't playing/queued" path.
pub fn any_tracked(state: &PlayerStateHandle, pred: impl Fn(i64) -> bool) -> bool {
    let g = lock_state(state);
    g.current_track().is_some_and(|t| pred(t.id)) || g.queue.tracks.iter().any(|t| pred(t.id))
}

/// Overwrite every queued / currently-playing [`TrackSummary`] whose id appears
/// in `fresh` with its fresh copy. Sibling of [`sync_current_track_if_in`], but
/// also walks `queue.tracks` — a tag edit changes exactly the title/artist/album
/// fields the Queue Sheet and Up Next render, not just the Now-Playing bar.
///
/// Pre-checks membership outside the emit lock so an edit touching nothing
/// queued/playing skips the publish entirely (the common case).
pub fn sync_track_summaries<S: std::hash::BuildHasher>(
    state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    fresh: &HashMap<i64, TrackSummary, S>,
) {
    if !any_tracked(state, |id| fresh.contains_key(&id)) {
        return;
    }

    with_state_emit(state, sinks, |s| {
        if let Some(track) = s.current_track_mut()
            && let Some(f) = fresh.get(&track.id)
        {
            *Arc::make_mut(track) = f.clone();
        }

        let mut queue_touched = false;
        for track in &mut s.queue.tracks {
            if let Some(f) = fresh.get(&track.id) {
                *Arc::make_mut(track) = f.clone();
                queue_touched = true;
            }
        }
        // A field-level `Arc::make_mut` doesn't advance the queue version on its
        // own, but `with_state_emit` only republishes the queue view-model when
        // the version changed — so bump it whenever a queued entry was patched,
        // or the Queue Sheet / Up Next would keep the stale summary.
        if queue_touched {
            s.queue.version += 1;
        }

        Vec::<PlayerAction>::new()
    });
}

/// Restore queue from persisted data. Called at startup via
/// `library::queue::restore_persisted_playback`.
///
/// `shuffle_enabled` and `repeat_mode` are user preferences and live in
/// `settings.json`, not `queue.json` — the caller is responsible for
/// hydrating them. Note that `original_order` is not persisted, so a
/// caller restoring a non-empty queue should also force shuffle off.
pub fn restore_queue(
    state: &mut PlayerState,
    tracks: Vec<Arc<TrackSummary>>,
    persistable: &PersistableQueue,
) {
    let len = tracks.len();
    state.queue.tracks = tracks;
    state.queue.play_order = (0..len).collect();
    state.queue.original_order = (0..len).collect();
    state.queue.current_index = super::queue::current_index_from_i32(persistable.current_index);

    if let Some(track) = state.queue.get_current().cloned() {
        state.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
        state.position_ms = u64::try_from(track.last_position.max(0)).unwrap_or(0);
        state.source = Some(PlaybackSource::Track(track));
    }
}

/// Put the station the last session was tuned to back on the deck, over the queue
/// [`restore_queue`] has already restored. Called from the same startup path.
///
/// `Paused` because that is already the one status holding a station with no connection —
/// pausing one drops its socket, so a restart is the same shape, and
/// `library::playback::player_play` re-opens from it. Seating the station is what evicts whatever
/// [`restore_queue`] just put on the deck, the two being one field.
///
/// Returns actions, so the caller owes an `emit_and_execute`: the speed pin is a backend write,
/// and boot is the one place a station can arrive over a rate `settings.json` restored.
pub fn restore_station(
    state: &mut PlayerState,
    station: Arc<RadioNowPlaying>,
) -> Vec<PlayerAction> {
    state.source = Some(PlaybackSource::Station(station));
    state.status = PlaybackStatus::Paused;
    state.duration_ms = 0;
    state.position_ms = 0;
    state.pin_speed_for_station().into_iter().collect()
}

/// Widen a u64 millisecond position to f64 for ratio math. Audio durations
/// stay well below 2^53 ms, so the conversion is lossless in practice.
#[allow(
    clippy::cast_precision_loss,
    reason = "ms positions are < 2^53; widening to f64 is lossless for any real audio duration"
)]
fn ms_to_f64(ms: u64) -> f64 {
    ms as f64
}

#[cfg(test)]
#[path = "tests/state_tests.rs"]
mod tests;
