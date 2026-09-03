//! The lock-free cells the transport does not touch: the graphic EQ, `ReplayGain`, crossfade and
//! the visualizer's tap.
//!
//! Every one of these is a pass-through to a shared cell an `EqSource` polls on its next sample, so
//! none takes the decks lock, bumps the epoch, or has anything to say about which voice is playing.
//! They sit here rather than beside the transport for exactly that reason — a reader following a
//! race through `mod.rs` should not have to walk past seventeen setters that cannot be in one.

use std::sync::Arc;

use super::PlaybackEngine;
use crate::player::playback::crossfade;
use crate::player::playback::replaygain::RgMode;
use crate::player::playback::visualizer::VisualizerShared;

impl PlaybackEngine {
    /// Enable / disable the graphic equalizer. Lock-free — every `EqSource`
    /// (playing + preloaded) picks the change up on its next sample.
    pub fn set_eq_enabled(&self, enabled: bool) {
        self.eq.set_enabled(enabled);
    }

    /// Set a single band's gain (dB). Out-of-range indices are ignored.
    pub fn set_eq_band(&self, index: usize, gain_db: f32) {
        self.eq.set_gain(index, gain_db);
    }

    /// Replace all band gains at once (preset / reset / boot hydration).
    pub fn set_eq_gains(&self, gains: &[f32]) {
        self.eq.set_all_gains(gains);
    }

    /// Set the EQ preamp / master gain (dB).
    pub fn set_eq_preamp(&self, preamp_db: f32) {
        self.eq.set_preamp(preamp_db);
    }

    /// Enable / disable `ReplayGain`. Lock-free, like the EQ setters.
    pub fn set_replaygain_enabled(&self, enabled: bool) {
        self.rg.set_enabled(enabled);
    }

    pub fn set_replaygain_mode(&self, mode: RgMode) {
        self.rg.set_mode(mode);
    }

    /// Set the `ReplayGain` preamp (dB).
    pub fn set_replaygain_preamp(&self, preamp_db: f32) {
        self.rg.set_preamp(preamp_db);
    }

    /// Enable / disable the static peak-based clip guard.
    pub fn set_replaygain_prevent_clipping(&self, on: bool) {
        self.rg.set_prevent_clipping(on);
    }

    /// Arm / disarm the visualizer's sample tap; while off it never touches the
    /// ring. The UI does not come through here — it already holds the cell via
    /// [`Self::visualizer`] for snapshotting and arms it off the Now-Playing
    /// view's visibility. This spelling exists for the crossfade integration test.
    pub fn set_visualizer_enabled(&self, on: bool) {
        self.viz.set_enabled(on);
    }

    /// The visualizer's sample ring, for the UI-side analyzer to snapshot.
    pub fn visualizer(&self) -> Arc<VisualizerShared> {
        self.viz.clone()
    }

    /// Snapshot the live crossfade settings for the playback monitor's decision.
    pub fn crossfade_settings(&self) -> crossfade::CrossfadeSettings {
        self.xf.snapshot()
    }

    /// Enable / disable crossfade. Lock-free, like the EQ setters.
    pub fn set_crossfade_enabled(&self, enabled: bool) {
        self.xf.set_enabled(enabled);
    }

    /// Set the crossfade length (ms), clamped to the supported range.
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
