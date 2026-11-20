//! The DSP chain and the device under it.
//!
//! Fixed order, innermost first, and all of it above [`output`]: `ReplayGain` pre-gain, the
//! headphone correction, the EQ bands, the crossfeed, the limiter, the clamp, the crossfade ramp,
//! then the visualizer tap. [`output`] is everything below that: the cpal stream, the two voices,
//! rate and channel conversion, the clock. [`decks`] is what `engine` takes off it.
//!
//! Each module argues its own place in that order and its own numbers.

pub mod correction;
pub mod crossfade;
pub mod crossfeed;
pub mod decks;
pub mod dsp;
pub mod equalizer;
mod limiter;
pub mod output;
pub mod replaygain;
pub mod spectrum;
pub mod stream_health;
pub mod visualizer;
pub mod waveform;

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod helpers;
}
