use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::MutexGuard;

use serde::{Deserialize, Serialize};

use super::event_sink::PlayerSinks;
use super::queue::QueueState;
use super::replaygain::TrackReplayGain;
use crate::entities::track::TrackSummary;

use super::types::{PersistableQueue, PlaybackStatus, RepeatMode};

/// Restart-from-beginning threshold for Previous command (ms).
pub const RESTART_THRESHOLD_MS: u64 = 3000;
/// Maximum volume level (percent) stored in `PlayerState` and reachable from the
/// UI. Playback amplitude tops out at unity gain (see [`volume_to_amplitude`]),
/// so this is the true ceiling — there is no boost band above it.
pub const MAX_VOLUME: u32 = 100;
/// Minimum playback speed multiplier.
pub const MIN_SPEED: f64 = 0.25;
/// Maximum playback speed multiplier. Capped at 2× — rodio's `set_speed` is
/// naive resampling (it shifts pitch), so beyond 2× the audio degrades into
/// chipmunk territory with little practical use for music.
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
    pub current_track: Option<Arc<TrackSummary>>,
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
    pub queue: QueueState,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            current_track: None,
            position_ms: 0,
            duration_ms: 0,
            volume: 100,
            is_muted: false,
            pre_mute_volume: 100,
            playback_speed: 1.0,
            gapless_enabled: true,
            pause_after_current_track: false,
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
    /// their rodio side effects on separate workers and leave state and backend
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
    /// (mirrors [`lock_state`] / `RodioPlayer::lock_player`). The guarded unit
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
/// `ui/models.slint`; the shape must match exactly across the boundary, so
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
    Pause,
    /// `fade_ms` is `0` for an internal stop (end of queue, error recovery) and
    /// the pause-fade length for a user-initiated stop.
    Stop {
        fade_ms: u64,
    },
    Seek {
        position_ms: u64,
    },
    SetVolume(f64),
    SetSpeed(f64),
    PreloadGapless(Option<String>),
    UpdatePlayCount(i64),
    UpdateSkipCount(i64),
    SavePosition {
        track_id: i64,
        position_ms: u64,
    },
}

impl PlayerState {
    fn progress_percent(&self) -> f64 {
        if self.duration_ms > 0 {
            (ms_to_f64(self.position_ms) / ms_to_f64(self.duration_ms)) * 100.0
        } else {
            0.0
        }
    }

    fn has_next(&self) -> bool {
        self.queue.peek_next().is_some()
    }

    fn has_previous(&self) -> bool {
        !self.queue.play_order.is_empty()
            && (self.queue.current_index.is_some_and(|ci| ci > 0)
                || self.queue.repeat_mode.wraps())
    }

    /// Full `ViewModel` — kept only for tests that assert against the
    /// composed state. Production traffic flows through `to_view_model_light`
    /// plus `to_queue_view_model` so the queue projection is rebuilt only
    /// when the queue actually changes, not on every player-state emit.
    #[cfg(test)]
    pub fn to_view_model(&self) -> PlayerViewModel {
        PlayerViewModel {
            status: self.status.as_str().to_owned(),
            current_track: self.current_track.clone(),
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            progress_percent: self.progress_percent(),
            volume: self.volume,
            is_muted: self.is_muted,
            playback_speed: self.playback_speed,
            gapless_enabled: self.gapless_enabled,
            sleep_at_track_end: self.pause_after_current_track,
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
            current_track: self.current_track.clone(),
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            progress_percent: self.progress_percent(),
            volume: self.volume,
            is_muted: self.is_muted,
            playback_speed: self.playback_speed,
            gapless_enabled: self.gapless_enabled,
            sleep_at_track_end: self.pause_after_current_track,
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

    /// Convert this state's volume for the audio backend: stored `[0, MAX_VOLUME]`,
    /// backend gets `[0.0, 1.0]`. Returns 0.0 when muted. Thin wrapper over the
    /// shared [`volume_to_amplitude`] so the MPRIS path can reuse the same math.
    pub fn effective_volume(&self) -> f64 {
        volume_to_amplitude(self.volume, self.is_muted)
    }

    /// Build actions for play/resume command.
    pub fn build_play_actions(&mut self) -> Vec<PlayerAction> {
        if self.status == PlaybackStatus::Paused {
            self.status = PlaybackStatus::Playing;
            vec![PlayerAction::Resume]
        } else {
            resume_from_stopped(self)
        }
    }

    /// Build actions for pause command.
    pub fn build_pause_actions(&mut self) -> Vec<PlayerAction> {
        if self.status == PlaybackStatus::Playing {
            self.status = PlaybackStatus::Paused;
            vec![PlayerAction::Pause]
        } else {
            vec![]
        }
    }

    /// Build actions for user-initiated stop (preserves position for resume).
    /// `fade_ms` is the pause-fade length when that setting is on, else `0`.
    pub fn build_stop_actions(&mut self, fade_ms: u64) -> Vec<PlayerAction> {
        self.status = PlaybackStatus::Stopped;
        vec![PlayerAction::Stop { fade_ms }]
    }

    /// Build actions for seek command.
    pub fn build_seek_actions(&mut self, position_ms: u64) -> Vec<PlayerAction> {
        self.position_ms = position_ms;
        vec![PlayerAction::Seek { position_ms }]
    }

    /// Build actions for next-track command.
    pub fn build_next_actions(&mut self) -> Vec<PlayerAction> {
        let mut actions = vec![];
        let was_paused = self.status == PlaybackStatus::Paused;

        if let Some(ref track) = self.current_track
            && self.duration_ms > 0
            && self.position_ms < self.duration_ms / 2
        {
            actions.push(PlayerAction::UpdateSkipCount(track.id));
        }

        if let Some(track) = self.queue.advance_skip().cloned() {
            actions.extend(play_track_inner(self, track, None));
            if was_paused {
                self.status = PlaybackStatus::Paused;
                actions.push(PlayerAction::Pause);
            }
        } else {
            actions.extend(stop_end_of_queue(self));
        }

        actions
    }

    /// Build actions for previous-track command.
    pub fn build_previous_actions(&mut self) -> Vec<PlayerAction> {
        let was_paused = self.status == PlaybackStatus::Paused;

        if self.position_ms > RESTART_THRESHOLD_MS {
            self.position_ms = 0;
            return vec![PlayerAction::Seek { position_ms: 0 }];
        }

        if let Some(track) = self.queue.previous().cloned() {
            let mut actions = play_track_inner(self, track, None);
            if was_paused {
                self.status = PlaybackStatus::Paused;
                actions.push(PlayerAction::Pause);
            }
            actions
        } else {
            self.position_ms = 0;
            vec![PlayerAction::Seek { position_ms: 0 }]
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
        let mut actions = Vec::with_capacity(4);

        if let Some(ref track) = self.current_track {
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
    /// there is no longer a next track.
    pub fn build_crossfade_actions(&mut self, fade_ms: u64) -> Vec<PlayerAction> {
        let mut actions = Vec::with_capacity(2);

        // The outgoing track counts as played the moment it starts fading. Same
        // accounting as `build_end_of_stream_actions`, just a few seconds early.
        let outgoing_id = self.current_track.as_ref().map(|t| t.id);

        // Re-read the queue under the emit lock rather than trusting the
        // monitor's earlier `peek_next` — a skip could have landed in between.
        let Some(track) = self.queue.advance().cloned() else {
            return actions;
        };

        if let Some(id) = outgoing_id {
            actions.push(PlayerAction::UpdatePlayCount(id));
        }

        self.status = PlaybackStatus::Playing;
        self.position_ms = 0;
        self.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
        let file_path = track.file_path.clone();
        let replaygain = track.replaygain();
        let volume = self.effective_volume();
        let speed = self.playback_speed;
        self.current_track = Some(track);

        actions.push(PlayerAction::BeginCrossfade {
            file_path,
            replaygain,
            fade_ms,
            volume,
            speed,
        });
        actions
    }

    /// Build actions for set-playback-speed command.
    pub fn build_set_speed_actions(&mut self, speed: f64) -> Vec<PlayerAction> {
        let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
        self.playback_speed = speed;
        vec![PlayerAction::SetSpeed(speed)]
    }
}

/// Core playback logic — reused by commands, bus handler, position poller.
/// Returns Vec<PlayerAction> for the caller to execute after releasing the state lock.
/// `start_position_ms` — if `Some`, seeks to that position after starting playback (used for resume).
pub fn play_track_inner(
    state: &mut PlayerState,
    track: Arc<TrackSummary>,
    start_position_ms: Option<u64>,
) -> Vec<PlayerAction> {
    state.status = PlaybackStatus::Playing;
    state.duration_ms = u64::try_from(track.duration_ms.max(0)).unwrap_or(0);
    // Clamp to 500ms before end to avoid immediate EOS detection by the playback monitor.
    let max_resume_pos = state.duration_ms.saturating_sub(500);
    let clamped_pos = start_position_ms
        .map(|p| p.min(max_resume_pos))
        .filter(|&p| p > 0);
    state.position_ms = clamped_pos.unwrap_or(0);
    let file_path = track.file_path.clone();
    let volume = state.effective_volume();
    let speed = state.playback_speed;
    let replaygain = track.replaygain();
    state.current_track = Some(track);

    // Gapless preload is staged late (by the playback monitor) when the
    // current track approaches its end — see `spawn_playback_monitor`. That
    // way mid-track repeat-mode / queue changes are reflected in what gets
    // preloaded, instead of being clobbered by a stale Rodio queue entry.
    vec![PlayerAction::PlayMedia {
        file_path,
        volume,
        speed,
        start_position_ms: clamped_pos,
        replaygain,
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
/// Returns empty vec if no track is loaded or status is not Stopped.
pub fn resume_from_stopped(state: &mut PlayerState) -> Vec<PlayerAction> {
    if state.status != PlaybackStatus::Stopped {
        return vec![];
    }
    if let Some(track) = state.current_track.clone() {
        let resume_pos = (state.position_ms > 0).then_some(state.position_ms);
        play_track_inner(state, track, resume_pos)
    } else {
        vec![]
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
        g.current_track.as_ref().is_some_and(|t| ids.contains(&t.id))
    };
    if !affects_current {
        return;
    }
    with_state_emit(state, sinks, |s| {
        // Re-check the id under the emit lock — the track may have advanced
        // between the pre-check and here.
        if let Some(track) = s.current_track.as_mut()
            && ids.contains(&track.id)
        {
            apply(Arc::make_mut(track));
        }
        Vec::<PlayerAction>::new()
    });
}

/// Restore queue from persisted data. Shared by startup (lib.rs) and `queue_load` command.
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
        state.current_track = Some(track);
    }
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
