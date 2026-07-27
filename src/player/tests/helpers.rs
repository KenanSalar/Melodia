//! Shared fixtures for the player's DSP tests.
//!
//! Three things were being written out more than once, so each lives here:
//! [`TestSource`] and its `NonZero` wrappers, which the equalizer and
//! visualizer suites both drain to compare a source's output against its input;
//! the float comparisons, which four suites need to check a value that is exact
//! in principle but rides through float maths; and [`fill_sine`], which the
//! spectrum and waveform suites both feed a known tone.
//!
//! The crossfade suite deliberately takes none of it: it exercises pure
//! predicates and the ramp cell, so it needs no source, and it holds a tighter
//! tolerance than [`approx_eq`] can — its gains are plain arithmetic on a
//! counter rather than the output of a filter.

use std::num::NonZero;
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

fn nz_u16(v: u16) -> ChannelCount {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u16>::MIN,
    }
}

fn nz_u32(v: u32) -> SampleRate {
    match NonZero::new(v) {
        Some(n) => n,
        None => NonZero::<u32>::MIN,
    }
}

/// Scalar near-equality — avoids `clippy::float_cmp` on checks that are exact
/// in principle (clamp bounds, preset values) but ride through float maths.
pub(crate) fn approx_eq(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// [`approx_eq`] as an assertion, reporting both sides on failure.
pub(crate) fn assert_approx(a: f32, b: f32) {
    assert!(approx_eq(a, b), "expected {b}, got {a}");
}

/// Bit pattern of each sample — lets a test assert *bit-identical* passthrough
/// (and divergence) without a float `==`.
pub(crate) fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

/// Fill `buf` with a sine of `freq_hz` at `sample_rate`, scaled to `amplitude`.
pub(crate) fn fill_sine(buf: &mut [f32], freq_hz: f32, sample_rate: f32, amplitude: f32) {
    for (i, sample) in buf.iter_mut().enumerate() {
        let phase = 2.0 * std::f32::consts::PI * freq_hz * crate::player::dsp::index_to_f32(i)
            / sample_rate;
        *sample = amplitude * phase.sin();
    }
}

/// In-memory source. `try_seek` rewinds to the start, like a decoder seeking to
/// 0, so a post-seek run can be compared against a fresh one.
pub(crate) struct TestSource {
    data: Vec<f32>,
    pos: usize,
    channels: u16,
    sample_rate: u32,
}

impl TestSource {
    pub(crate) fn new(data: Vec<f32>, channels: u16, sample_rate: u32) -> Self {
        Self { data, pos: 0, channels, sample_rate }
    }
}

impl Iterator for TestSource {
    type Item = Sample;
    fn next(&mut self) -> Option<Sample> {
        let s = self.data.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for TestSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        nz_u16(self.channels)
    }
    fn sample_rate(&self) -> SampleRate {
        nz_u32(self.sample_rate)
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        self.pos = 0;
        Ok(())
    }
}
