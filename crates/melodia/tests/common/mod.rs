//! What both mixer-driving integration binaries need.
//!
//! `src/player/tests/helpers.rs` already holds these for the unit suites, but a `#[cfg(test)]`
//! module is not reachable from an integration binary, so this is where the same two answers live
//! for the other side of the wall. Each binary declares `mod common;`, which compiles it into that
//! binary rather than into a test target of its own.

use std::num::NonZero;

use melodia_audio::player::source::audio::{Sample, Shape};
use melodia_playback::player::playback::output::mixer::MixerPull;

/// A [`Shape`] from the plain integers a test spells.
///
/// The `NonZero` floors are unreachable — every caller passes a literal — and they are what keeps
/// the fixture free of an `unwrap` the tree does not allow anywhere, tests included.
pub fn shape(channels: u16, rate: u32) -> Shape {
    Shape {
        channels: NonZero::new(channels).unwrap_or(NonZero::<u16>::MIN),
        rate: NonZero::new(rate).unwrap_or(NonZero::<u32>::MIN),
    }
}

/// Pull `samples` interleaved samples as the output callback would.
///
/// The mixer never ends — an idle voice contributes nothing rather than stopping the block — so a
/// short read is not something a caller has to handle.
pub fn pull(mixer: &mut MixerPull, samples: usize) -> Vec<Sample> {
    let mut block = vec![0.0; samples];
    mixer.fill(&mut block);
    block
}
