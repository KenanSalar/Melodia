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

/// The rate of [`frame_indexed_wav`], at which a cue frame is 640 samples.
pub(crate) const INDEXED_RATE: u32 = 48_000;

/// Writes a mono 16-bit PCM WAV of `frames` frames at [`INDEXED_RATE`], each sample holding its own
/// frame index, so a decoded sample says which frame of the file it is.
pub(crate) fn frame_indexed_wav(path: &Path, frames: i16) -> std::io::Result<()> {
    let samples: Vec<i16> = (0..frames).collect();
    pcm_wav(path, 1, INDEXED_RATE, &samples)
}

/// Writes `samples`, interleaved over `channels`, as a 16-bit PCM WAV at `rate`.
pub(crate) fn pcm_wav(
    path: &Path,
    channels: u16,
    rate: u32,
    samples: &[i16],
) -> std::io::Result<()> {
    let data_len = u32::try_from(samples.len() * 2).map_err(std::io::Error::other)?;
    let block_align = channels * 2;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_len).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(channels.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * u32::from(block_align)).to_le_bytes());
    bytes.extend(block_align.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_len.to_le_bytes());
    for sample in samples {
        bytes.extend(sample.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

/// Whether `sample` is what [`frame_indexed_wav`] holds at `frame`, to within a quarter of a 16-bit
/// step: the decoder scales a sample to `[-1, 1)` by 32,768.
pub(crate) fn is_frame(sample: Option<&f32>, frame: i16) -> bool {
    sample.is_some_and(|sample| (sample - f32::from(frame) / 32_768.0).abs() < 0.25 / 32_768.0)
}
