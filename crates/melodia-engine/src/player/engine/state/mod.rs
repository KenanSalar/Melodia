use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{MutexGuard, TryLockError};

use super::event_sink::PlayerSinks;
use super::queue::{ClearedQueue, QueueCut, QueueEntry, QueueState};
use super::queue_window::TrackWindow;
use super::types::{PlaybackSource, PlaybackStatus, RadioNowPlaying};
use melodia_core::entities::track::TrackSummary;

mod action;
mod persistence;
mod station;
mod summary_sync;
mod transport;
mod view_model;

pub use action::{AwaitedStart, PlayerAction};
pub use persistence::{restore_queue, restore_station};
pub use summary_sync::{WindowFill, seat_window_records, sync_current_track_if_in, window_wanted};
pub use transport::{ReleaseDecision, play_track_inner, resume_from_stopped, stop_end_of_queue};
pub use view_model::{PlayerViewModel, PlayerViewModelLight, PositionTick, QueueViewModel};

/// Restart-from-beginning threshold for Previous command (ms).
pub const RESTART_THRESHOLD_MS: u64 = 3000;
/// Percent, and the true ceiling: amplitude tops out at unity, with no boost band above it.
pub const MAX_VOLUME: u32 = 100;
pub const MIN_SPEED: f64 = 0.25;
/// Capped at 2×: speed is plain resampling, so it shifts pitch with it.
pub const MAX_SPEED: f64 = 2.0;

/// The one conversion from a stored volume and mute flag to the amplitude the backend and MPRIS
/// both take.
pub fn volume_to_amplitude(volume: u32, is_muted: bool) -> f64 {
    if is_muted { 0.0 } else { f64::from(volume) / 100.0 }
}

/// The way back, for an OS media panel or the device's own control. Clamped, since a host may
/// send anything, and rounded so the quietest step is reachable from half a percent.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to [0, 1] before the scale, so the rounded value fits a u32"
)]
pub fn amplitude_to_volume(amplitude: f64) -> u32 {
    (amplitude.clamp(0.0, 1.0) * 100.0).round() as u32
}

pub struct PlayerState {
    pub status: PlaybackStatus,
    pub source: Option<PlaybackSource>,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Orders every published position across the two channels it travels on, so
    /// `ui::shell::bridge::ShownPosition` can drop the older of two arriving out of order.
    pub position_stamp: u64,
    pub volume: u32,
    pub is_muted: bool,
    pub pre_mute_volume: u32,
    pub playback_speed: f64,
    pub gapless_enabled: bool,
    /// The sleep timer's end-of-track mode: the monitor stops at the next end of stream instead
    /// of advancing, disarming it. Session-only.
    pub pause_after_current_track: bool,
    /// Bumped by every transition that starts or ends a station session, so a connect finishing
    /// after the user moved on is refused rather than played late.
    pub radio_generation: u64,
    pub queue: QueueState,
    /// The records around the queue's current slot, all a transition reads beyond the ids.
    pub window: TrackWindow,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            source: None,
            position_ms: 0,
            duration_ms: 0,
            position_stamp: 0,
            volume: 100,
            is_muted: false,
            pre_mute_volume: 100,
            playback_speed: 1.0,
            gapless_enabled: true,
            pause_after_current_track: false,
            radio_generation: 0,
            queue: QueueState::default(),
            window: TrackWindow::default(),
        }
    }
}

/// `PlayerState` behind its mutex, with a status mirror so an idle monitor skips the lock.
pub struct PlayerStateHandle {
    mutex: std::sync::Mutex<PlayerState>,
    pub status_atomic: AtomicU8,
    /// Held by `emit_and_execute` across a mutation and its side effects, so mutation order is
    /// side-effect order across tasks. Never held across an `.await`.
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
    /// Recovers from poison: the guard carries no data, only the order.
    pub fn lock_exec(&self) -> MutexGuard<'_, ()> {
        self.exec_lock.lock().unwrap_or_else(|poisoned| {
            log::error!("PlayerState exec lock was poisoned, recovering");
            poisoned.into_inner()
        })
    }

    /// [`Self::lock_exec`] for a caller that would rather skip than wait behind a transport op,
    /// `None` while one holds it.
    pub fn try_lock_exec(&self) -> Option<MutexGuard<'_, ()>> {
        match self.exec_lock.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(poisoned)) => {
                log::error!("PlayerState exec lock was poisoned, recovering");
                Some(poisoned.into_inner())
            }
        }
    }
}

impl PlayerState {
    /// The track on the deck, or `None` when the source is a live one or there is none.
    pub fn current_track(&self) -> Option<&Arc<TrackSummary>> {
        self.source.as_ref().and_then(PlaybackSource::track)
    }

    /// The track on the deck, for the two writers that mutate it in place.
    pub fn current_track_mut(&mut self) -> Option<&mut Arc<TrackSummary>> {
        self.source.as_mut().and_then(PlaybackSource::track_mut)
    }

    /// Takes the queue rows `entries` name. A seated track whose row goes plays on, the row that
    /// took its place next.
    pub fn cut_queue(&mut self, entries: &[QueueEntry]) -> Option<QueueCut> {
        let cut = self.queue.cut(entries);
        self.close_gap_unless_seated();
        cut
    }

    pub fn restore_queue_cuts(&mut self, cuts: Vec<QueueCut>) {
        self.queue.restore_cuts(cuts);
        self.close_gap_unless_seated();
    }

    pub fn restore_cleared_queue(&mut self, cleared: Vec<ClearedQueue>) {
        self.queue.restore_cleared(cleared);
        self.close_gap_unless_seated();
    }

    /// With no track seated nothing plays on to leave a gap for, so a removal or a restore that
    /// opened one closes it.
    fn close_gap_unless_seated(&mut self) {
        if self.current_track().is_none() {
            self.queue.close_gap();
        }
    }

    /// The record for queued track `id`, from the window or, where it is the one playing, the deck.
    pub fn record(&self, id: i64) -> Option<Arc<TrackSummary>> {
        self.window.get(id).or_else(|| self.current_track().filter(|track| track.id == id)).cloned()
    }

    /// The station on the deck, including through the stretch where it is still connecting.
    pub fn station(&self) -> Option<&Arc<RadioNowPlaying>> {
        self.source.as_ref().and_then(PlaybackSource::station)
    }

    pub fn station_mut(&mut self) -> Option<&mut Arc<RadioNowPlaying>> {
        self.source.as_mut().and_then(PlaybackSource::station_mut)
    }

    /// Only for questions about a station itself; everything else asks [`Self::source_allows`],
    /// so a third kind of source isn't taken for a file.
    fn is_radio(&self) -> bool {
        self.station().is_some()
    }

    /// An empty deck permits everything: a loaded queue with nothing on the deck still offers
    /// Next.
    pub fn source_allows(&self, capability: fn(&PlaybackSource) -> bool) -> bool {
        self.source.as_ref().is_none_or(capability)
    }

    /// Advance [`Self::position_stamp`] for a publish carrying `position_ms`.
    pub fn stamp_position(&mut self) {
        self.position_stamp += 1;
    }
}

/// Recovers from poison: a possibly inconsistent state beats a media player that crashes.
pub fn lock_state(handle: &PlayerStateHandle) -> MutexGuard<'_, PlayerState> {
    handle.mutex.lock().unwrap_or_else(|poisoned| {
        log::error!("PlayerState mutex was poisoned, recovering");
        poisoned.into_inner()
    })
}

/// Mutates the state and publishes its view model after dropping the lock, so no mutation can
/// forget to; the queue's only when its version moved. Paired with `execute_actions` only through
/// `emit_and_execute`.
pub fn with_state_emit<F, R>(state: &PlayerStateHandle, sinks: &PlayerSinks, f: F) -> R
where
    F: FnOnce(&mut PlayerState) -> R,
{
    let mut guard = lock_state(state);
    let queue_version_before = guard.queue.version;
    let result = f(&mut guard);
    guard.stamp_position();
    let status = guard.status;
    state.status_atomic.store(status as u8, Ordering::Relaxed);
    let vm_light = guard.to_view_model_light();
    let queue_vm = if guard.queue.version == queue_version_before {
        None
    } else {
        Some(guard.to_queue_view_model())
    };
    drop(guard);

    if let Some(qvm) = queue_vm {
        let _ = sinks.queue.send(Some(qvm));
    }
    if let Some(mc) = sinks.media_controls.as_ref() {
        mc.sync(&vm_light, status);
    }
    let _ = sinks.view_model.send(Some(vm_light));

    result
}

#[cfg(test)]
#[path = "../tests/state_tests.rs"]
mod tests;
