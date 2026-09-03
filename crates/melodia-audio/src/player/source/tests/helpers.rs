//! Fixtures for this tier's tests.
//!
//! A `#[cfg(test)]` module cannot be reached across a crate boundary, so what used to be one
//! shared file under `player/` is now one per tier. These three are the cheap half: constructors
//! over the tier's own public vocabulary, which each tier can spell for itself rather than any
//! of them growing a production export that exists only for tests.

use std::num::NonZero;

use crate::player::source::audio::{ChannelCount, SampleRate, Shape};

pub(crate) fn nz_u16(v: u16) -> ChannelCount {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u16>::MIN,
    }
}

pub(crate) fn nz_u32(v: u32) -> SampleRate {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u32>::MIN,
    }
}

/// A [`Shape`] from the plain integers a test spells, over the two above.
pub(crate) fn shape(channels: u16, rate: u32) -> Shape {
    Shape {
        channels: nz_u16(channels),
        rate: nz_u32(rate),
    }
}
