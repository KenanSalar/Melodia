//! The projections `with_state_emit` publishes, and the derived fields they share.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::PlayerState;
use crate::player::engine::queue::current_index_to_i32;
use crate::player::engine::types::{PlaybackSource, RadioNowPlaying, RepeatMode};
use melodia_core::entities::track::TrackSummary;

/// Lightweight event for 500ms position ticks — avoids serializing the full queue.
#[derive(Debug, Clone, Serialize)]
pub struct PositionTick {
    pub position_ms: u64,
    pub duration_ms: u64,
}

/// Full `ViewModel`, built only by the test-only `PlayerState::to_view_model`.
///
/// The bool fields mirror the Slint `PlayerVm`/`QueueVm` structs in
/// `crates/melodia-ui/ui/models.slint`; the shape must match exactly across the boundary, so
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
    /// Copied from the queue, as `has_next` is, so an OS media panel reads them off the model it
    /// is already synced with rather than waiting on a queue emit.
    pub shuffle_enabled: bool,
    pub repeat_mode: RepeatMode,
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

impl PlayerState {
    fn progress_percent(&self) -> f64 {
        if self.duration_ms > 0 {
            (ms_to_f64(self.position_ms) / ms_to_f64(self.duration_ms)) * 100.0
        } else {
            0.0
        }
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
            queue_index: current_index_to_i32(self.queue.current_index),
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
            shuffle_enabled: self.queue.shuffle_enabled,
            repeat_mode: self.queue.repeat_mode,
        }
    }

    /// Queue-only `ViewModel` — emitted only when the queue changes.
    pub fn to_queue_view_model(&self) -> QueueViewModel {
        QueueViewModel {
            queue_tracks: self.queue.tracks_in_play_order(),
            queue_index: current_index_to_i32(self.queue.current_index),
            shuffle_enabled: self.queue.shuffle_enabled,
            repeat_mode: self.queue.repeat_mode,
            has_next: self.has_next(),
            has_previous: self.has_previous(),
        }
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
