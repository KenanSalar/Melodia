//! The seam a test stands a mock in at.
//!
//! [`PlayerBackend`] is every transport op the action layer issues and nothing else — no monitor
//! query, no DSP setter — so `actions::execute_actions` can be driven against `MockBackend`
//! (`player/tests/actions_tests.rs`) with no audio device. The one impl below is
//! [`PlaybackEngine`]'s; callers hold an `Arc` and deref at the call site, which every one of them
//! now does. A blanket `impl<T: Deref>` used to spare three of them the `*` at the cost of forty
//! lines forwarding twelve signatures a third time.

use melodia_core::error::AppError;

use super::PlaybackEngine;
use melodia_playback::player::playback::replaygain::TrackReplayGain;

/// Audio playback operations, so tests can stand a mock in for `PlaybackEngine`.
pub trait PlayerBackend: Send + Sync {
    fn play_media(
        &self,
        file_path: &str,
        volume: f64,
        speed: f64,
        start_position_ms: Option<u64>,
        baked_rg: TrackReplayGain,
    ) -> Result<(), AppError>;
    /// Start the next track on the idle deck and cross-fade over `fade_ms`
    /// **media** milliseconds; the outgoing deck ends itself when its ramp lands.
    fn begin_crossfade(
        &self,
        file_path: &str,
        baked_rg: TrackReplayGain,
        fade_ms: u64,
        volume: f64,
        speed: f64,
    ) -> Result<(), AppError>;
    fn resume(&self);
    /// Fade to silence over `fade_ms` and then pause. `0` is an immediate pause.
    /// `PlayerAction::Pause` always carries a length, so the action layer needs
    /// no unconditional `pause()` beside this.
    fn pause_with_fade(&self, fade_ms: u64);
    fn stop(&self);
    /// Fade to silence over `fade_ms` and then stop. `0` is an immediate stop.
    fn stop_with_fade(&self, fade_ms: u64);
    /// Move the playing track to `position_ms`. Takes the file and its gain because the seek is
    /// done by building a source already there; see `PlaybackEngine::seek`.
    fn seek(&self, file_path: &str, position_ms: u64, baked_rg: TrackReplayGain);
    fn set_volume(&self, volume: f64);
    fn set_speed(&self, speed: f64);
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain);
    /// Start the live stream staged under `generation`, hard-cutting whatever was playing.
    ///
    /// Takes no path and no `ReplayGain`: the stream was opened asynchronously long before this
    /// runs (a socket has no business on the action executor's thread), and a live source carries
    /// no per-track tags to bake. Fails when nothing is staged under that generation, which is how
    /// a station superseded mid-connect is refused rather than played late.
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
    fn preload_gapless(&self, file_path: Option<&str>, baked_rg: TrackReplayGain) {
        self.preload_gapless(file_path, baked_rg);
    }
    fn play_stream(&self, generation: u64, volume: f64) -> Result<(), AppError> {
        self.play_stream(generation, volume)
    }
}
