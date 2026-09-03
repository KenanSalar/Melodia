//! Everything from the DSP chain down to the device: the EQ, `ReplayGain`, the limiter,
//! crossfade, the visualizer taps, the decks, and `playback::output` under all of them.
//!
//! Names cpal and never the network, which is the inverse of `melodia-audio` and the property
//! the two-crate split exists to keep. `output` has a narrower meaning than this crate — it is
//! everything *under* the DSP chain — so it stays nested rather than widened.

pub use melodia_core::{config, entities, error, themes, utils};

pub mod player {
    pub use melodia_audio::player::source;

    pub mod playback;
}

#[cfg(test)]
pub(crate) use melodia_testkit as test_support;
