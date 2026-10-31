//! The seam a test stands a mock in at: every transport op the action layer issues and nothing
//! else, so `execute_actions` runs against `MockBackend` with no audio device.

use melodia_core::error::AppError;

use super::PlaybackEngine;
use melodia_playback::player::playback::replaygain::TrackReplayGain;

pub trait PlayerBackend: Send + Sync {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError>;
    fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError>;
    fn resume(&self);
    /// `0` pauses at once, so the action layer needs no bare `pause()`.
    fn pause_with_fade(&self, fade_ms: u64);
    fn stop(&self);
    /// `0` stops at once.
    fn stop_with_fade(&self, fade_ms: u64);
    fn seek(&self, file_path: &str, position_ms: u64, baked_rg: TrackReplayGain);
    fn set_volume(&self, volume: f64);
    fn set_speed(&self, speed: f64);
    /// Fails where nothing is staged under `generation`, which is how a superseded station is
    /// refused rather than played late.
    fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError>;
}

impl PlayerBackend for PlaybackEngine {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError> {
        self.play_media(file_path, volume, speed, start_position_ms, baked_rg)
    }
    fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError> {
        self.begin_crossfade(file_path, baked_rg, fade_ms, volume, speed)
    }
    fn resume(&self) {
        self.resume();
    }
    fn pause_with_fade(&self, fade_ms: u64) {
        self.pause_with_fade(fade_ms);
    }
    fn stop(&self) {
        self.stop();
    }
    fn stop_with_fade(&self, fade_ms: u64) {
        self.stop_with_fade(fade_ms);
    }
    fn seek(&self, file_path: &str, position_ms: u64, baked_rg: TrackReplayGain) {
        self.seek(file_path, position_ms, baked_rg);
    }
    fn set_volume(&self, volume: f64) {
        self.set_volume(volume);
    }
    fn set_speed(&self, speed: f64) {
        self.set_speed(speed);
    }
    fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError> {
        self.play_stream(generation, volume)
    }
}
