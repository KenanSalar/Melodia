//! The projections `with_state_emit` publishes, and the derived fields they share.

use std::sync::Arc;

use super::PlayerState;
use crate::player::engine::queue::current_index_to_i32;
use crate::player::engine::types::{PlaybackSource, RadioNowPlaying, RepeatMode};
use melodia_core::entities::track::TrackSummary;

/// The 500 ms position tick, on its own channel so a tick doesn't republish the view model.
#[derive(Debug, Clone)]
pub struct PositionTick {
    pub position_ms: u64,
    pub duration_ms: u64,
    /// [`PlayerState::position_stamp`] at the read, for ordering against the view model.
    pub stamp: u64,
}

/// Built only by the test-only `PlayerState::to_view_model`. The bools mirror the Slint structs in
/// `crates/melodia-ui/ui/models.slint` exactly, so they can't collapse into bitflags.
#[allow(
    clippy::struct_excessive_bools,
    reason = "ViewModel mirrors Slint struct shape across the FFI boundary"
)]
#[derive(Debug, Clone)]
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
    pub queue_ids: Arc<[i64]>,
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
#[derive(Debug, Clone)]
pub struct PlayerViewModelLight {
    pub status: &'static str,
    pub current_track: Option<Arc<TrackSummary>>,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// [`PlayerState::position_stamp`] at the emit, for ordering against the position ticks.
    pub position_stamp: u64,
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

/// Emitted only when the queue changes. It carries no
/// [`QueueEntry`](crate::player::engine::queue::QueueEntry)s: the latest one stays alive for every
/// install, so the panel takes them through `QueueState::play_entries` while it is shown.
#[derive(Debug, Clone)]
pub struct QueueViewModel {
    /// The queue in play order, shared with the state until the order moves: an advance publishes
    /// the same allocation it published last.
    pub ids: Arc<[i64]>,
    /// The row shown as playing, `-1` with none.
    pub queue_index: i32,
    /// Where Up Next starts: the row after `queue_index`, or with the playing track gone from the
    /// queue, the row that took its place.
    pub upcoming_from: usize,
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
        self.source_allows(PlaybackSource::advances_queue) && self.queue.has_next()
    }

    fn has_previous(&self) -> bool {
        self.source_allows(PlaybackSource::advances_queue) && self.queue.has_previous()
    }

    /// Production publishes the light and queue halves, so the queue is rebuilt only when it moves.
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
            queue_ids: self.queue.play_ids(),
            queue_index: current_index_to_i32(self.queue.playing_index()),
            shuffle_enabled: self.queue.shuffle_enabled,
            repeat_mode: self.queue.repeat_mode,
            has_next: self.has_next(),
            has_previous: self.has_previous(),
        }
    }

    pub fn to_view_model_light(&self) -> PlayerViewModelLight {
        PlayerViewModelLight {
            status: self.status.as_str(),
            current_track: self.current_track().cloned(),
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            position_stamp: self.position_stamp,
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

    pub fn to_queue_view_model(&self) -> QueueViewModel {
        QueueViewModel {
            ids: self.queue.play_ids(),
            queue_index: current_index_to_i32(self.queue.playing_index()),
            upcoming_from: self.queue.upcoming_from(),
            shuffle_enabled: self.queue.shuffle_enabled,
            repeat_mode: self.queue.repeat_mode,
            has_next: self.has_next(),
            has_previous: self.has_previous(),
        }
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "ms positions are < 2^53; widening to f64 is lossless for any real audio duration"
)]
fn ms_to_f64(ms: u64) -> f64 {
    ms as f64
}
