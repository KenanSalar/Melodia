//! Fixtures for this tier's tests.
//!
//! A `#[cfg(test)]` module cannot be reached across a crate boundary, so what used to be one
//! shared file under `player/` is now one per tier. The three constructors are the cheap half:
//! the tier's own public vocabulary spelled from plain integers, rather than any tier growing a
//! production export that exists only for tests. [`asset`] is here for the plainer reason that
//! three suites were spelling it identically.

use std::num::NonZero;
use std::path::{Path, PathBuf};

use crate::player::source::audio::{ChannelCount, SampleRate, Shape};
use melodia_testkit::ASSETS_DIR;

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
    Shape { channels: nz_u16(channels), rate: nz_u32(rate) }
}

/// A fixture in `test-assets/` by name.
pub(crate) fn asset(name: &str) -> PathBuf {
    Path::new(ASSETS_DIR).join(name)
}
