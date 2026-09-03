//! The DSP chain and the device under it.
//!
//! Fixed order, innermost first, and all of it above [`output`]: `ReplayGain` pre-gain, the EQ
//! bands, the limiter, the clamp, the crossfade ramp, then the visualizer tap. [`output`] is
//! everything below that — the cpal stream, the two voices, rate and channel conversion, the
//! clock — and [`decks`] is what `engine` takes off it.
//!
//! `CLAUDE.md` beside `player/mod.rs` argues the chain; each module argues its own numbers.

pub mod crossfade;
pub mod decks;
pub mod dsp;
pub mod equalizer;
pub mod output;
pub mod replaygain;
pub mod spectrum;
pub mod stream_health;
pub mod visualizer;
pub mod waveform;
