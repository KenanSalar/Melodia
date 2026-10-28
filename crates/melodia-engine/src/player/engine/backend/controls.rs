//! The lock-free cells the transport doesn't touch: the EQ, `ReplayGain`, crossfade and the
//! visualizer's tap. Pass-throughs to cells an `EqSource` polls on its next sample, kept out of
//! `mod.rs` since none can be in a race.

use std::sync::Arc;

use super::PlaybackEngine;
use melodia_playback::player::playback::crossfade;
use melodia_playback::player::playback::replaygain::RgMode;
use melodia_playback::player::playback::visualizer::VisualizerShared;

impl PlaybackEngine {
    pub fn set_eq_enabled(&self, enabled: bool) {
        self.eq.set_enabled(enabled);
    }

    /// An out-of-range index is ignored.
    pub fn set_eq_band(&self, index: usize, gain_db: f32) {
        self.eq.set_gain(index, gain_db);
    }

    pub fn set_eq_gains(&self, gains: &[f32]) {
        self.eq.set_all_gains(gains);
    }

    pub fn set_eq_preamp(&self, preamp_db: f32) {
        self.eq.set_preamp(preamp_db);
    }

    pub fn set_replaygain_enabled(&self, enabled: bool) {
        self.rg.set_enabled(enabled);
    }

    pub fn set_replaygain_mode(&self, mode: RgMode) {
        self.rg.set_mode(mode);
    }

    pub fn set_replaygain_preamp(&self, preamp_db: f32) {
        self.rg.set_preamp(preamp_db);
    }

    /// The static peak-based clip guard.
    pub fn set_replaygain_prevent_clipping(&self, on: bool) {
        self.rg.set_prevent_clipping(on);
    }

    /// For the crossfade integration test; the UI arms the cell [`Self::visualizer`] hands it.
    pub fn set_visualizer_enabled(&self, on: bool) {
        self.viz.set_enabled(on);
    }

    pub fn visualizer(&self) -> Arc<VisualizerShared> {
        self.viz.clone()
    }

    pub fn crossfade_settings(&self) -> crossfade::CrossfadeSettings {
        self.xf.snapshot()
    }

    pub fn set_crossfade_enabled(&self, enabled: bool) {
        self.xf.set_enabled(enabled);
    }

    /// Clamped to the supported range.
    pub fn set_crossfade_duration_ms(&self, ms: u32) {
        self.xf.set_duration_ms(ms);
    }

    /// Also crossfade when the user changes track manually.
    pub fn set_crossfade_manual(&self, on: bool) {
        self.xf.set_manual(on);
    }

    /// Leave same-album transitions gapless.
    pub fn set_crossfade_skip_same_album(&self, on: bool) {
        self.xf.set_skip_same_album(on);
    }

    /// Fade out on pause / user stop, fade back in on resume.
    pub fn set_crossfade_fade_on_pause(&self, on: bool) {
        self.xf.set_fade_on_pause(on);
    }
}
