//! Playback, in the three tiers the audio stack cuts into. Contracts live in `CLAUDE.md` beside
//! this file; what belongs here is only which tier a module is in and why the order is that way.
//!
//! - [`source`]: the vocabulary the whole chain is written against ([`source::audio`]), one
//!   Symphonia behind [`source::decode`], and the four things that can feed it — a file, a live
//!   stream, its ring, and HLS. Names the network and never cpal.
//! - [`playback`]: everything from the DSP chain down to the device. The EQ, `ReplayGain`, the
//!   limiter, crossfade, the visualizer taps, the decks, and [`playback::output`] under all of
//!   them. Names cpal and never the network.
//! - [`engine`]: the state machine, the queue, the action list and the backend that runs them.
//!   Names neither.
//!
//! **The two lower tiers' dependency sets do not intersect**, and the direction never reverses:
//! `engine` reads `playback` reads `source`, and nothing points back up. That is what makes a new
//! source kind a new [`source::audio::AudioSource`] and nothing else — under one flat directory
//! it could reach the mixer and the state machine, and here it cannot.

pub mod engine;
pub mod playback;
pub mod source;

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) mod helpers;
}
