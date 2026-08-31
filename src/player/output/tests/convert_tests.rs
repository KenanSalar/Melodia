//! Tests for the rate and channel converter.
//!
//! The frame counts are exact rather than approximate, because the converter is what decides how
//! long a track is: a ratio that rounds the wrong way drifts against the clock counting the frames
//! it consumed, and the two are read against each other by every crossfade.

use super::{Converter, Filled, Shape};
use crate::player::tests::helpers::{TestSource, bits, nz_u16, nz_u32};

fn shape(channels: u16, rate: u32) -> Shape {
    Shape {
        channels: nz_u16(channels),
        rate: nz_u32(rate),
    }
}

/// `count` distinct, exactly representable samples, so an output can be traced back to its input.
fn ramp(count: usize) -> Vec<f32> {
    (0..count).map(|i| f32::from(u16::try_from(i).unwrap_or(u16::MAX)) / 1024.0).collect()
}

/// Drain a source through a converter into a buffer big enough for all of it.
fn drain(source: Vec<f32>, channels: u16, from: u32, to: u32, speed: f64) -> (Vec<f32>, u64) {
    let mut src = TestSource::new(source, channels, from);
    let device = shape(channels, to);
    let mut converter = Converter::new(shape(channels, from), device);

    let mut out = vec![0.0; 4096];
    let Filled {
        samples,
        source_frames,
    } = converter.fill(&mut out, &mut src, speed);
    out.truncate(samples);
    (out, source_frames)
}

#[test]
fn equal_rates_pass_every_sample_through_untouched() {
    let input = ramp(8);
    let (out, frames) = drain(input.clone(), 1, 44_100, 44_100, 1.0);

    assert_eq!(bits(&out), bits(&input), "an equal-rate fill must not touch a single sample");
    assert_eq!(frames, 8);
}

/// The passthrough has to survive the sign of zero, which is what a multiply-add loses: `-0.0` is
/// the sample a silent stretch of a signed format decodes to, and bit-perfect means bit-perfect.
#[test]
fn the_passthrough_keeps_negative_zero() {
    let (out, _) = drain(vec![-0.0, 0.0, -0.0], 1, 48_000, 48_000, 1.0);
    assert_eq!(bits(&out), bits(&[-0.0, 0.0, -0.0]));
}

#[test]
fn doubling_the_rate_doubles_the_frames() {
    let (out, frames) = drain(ramp(4), 1, 24_000, 48_000, 1.0);
    assert_eq!(out.len(), 8);
    assert_eq!(frames, 4);
}

#[test]
fn halving_the_rate_halves_the_frames() {
    let (out, frames) = drain(ramp(8), 1, 48_000, 24_000, 1.0);
    assert_eq!(out.len(), 4);
    assert_eq!(frames, 8);
}

/// The ratio every library hits and no device offers: 44.1 kHz material on a 48 kHz device.
#[test]
fn the_common_ratio_lands_within_a_frame_of_its_own_arithmetic() {
    let (out, frames) = drain(ramp(441), 1, 44_100, 48_000, 1.0);
    assert_eq!(frames, 441);
    let want = 441 * 48_000 / 44_100;
    assert!(out.len().abs_diff(want) <= 1, "got {} output frames, expected ~{want}", out.len());
}

/// Playback speed is the same ratio from the other side, so it must arrive at the same count.
#[test]
fn speed_scales_the_ratio_rather_than_the_reported_rate() {
    let (half, _) = drain(ramp(8), 1, 48_000, 48_000, 2.0);
    let (double, _) = drain(ramp(4), 1, 48_000, 48_000, 0.5);
    assert_eq!(half.len(), 4, "double speed consumes two source frames per output frame");
    assert_eq!(double.len(), 8, "half speed emits two output frames per source frame");
}

/// Dropping the final frame is a click at every gapless boundary, which is the one place a listener
/// would hear it — the tracks either side are meant to be continuous.
#[test]
fn the_last_source_frame_is_handed_over() {
    let input = ramp(5);
    let (out, _) = drain(input.clone(), 1, 44_100, 44_100, 1.0);
    assert_eq!(out.last().map(|s| s.to_bits()), input.last().map(|s| s.to_bits()));
}

#[test]
fn a_one_frame_source_yields_that_frame() {
    let (out, frames) = drain(vec![0.25], 1, 44_100, 44_100, 1.0);
    assert_eq!(bits(&out), bits(&[0.25]));
    assert_eq!(frames, 1);
}

#[test]
fn an_empty_source_yields_nothing() {
    let (out, frames) = drain(Vec::new(), 1, 44_100, 44_100, 1.0);
    assert!(out.is_empty());
    assert_eq!(frames, 0);
}

/// Mono into a stereo device is the case the mapping exists for: silence in the right channel is
/// what a naive copy gives, and it is what a listener reports as "it only plays on one side".
#[test]
fn a_mono_source_is_duplicated_across_a_stereo_device() {
    let mut src = TestSource::new(vec![0.5, 0.25], 1, 48_000);
    let mut converter = Converter::new(shape(1, 48_000), shape(2, 48_000));

    let mut out = vec![0.0; 4];
    let filled = converter.fill(&mut out, &mut src, 1.0);

    assert_eq!(filled.samples, 4);
    assert_eq!(bits(&out), bits(&[0.5, 0.5, 0.25, 0.25]));
}

#[test]
fn a_wider_source_has_its_extra_channels_dropped() {
    let mut src = TestSource::new(vec![0.5, 0.25], 2, 48_000);
    let mut converter = Converter::new(shape(2, 48_000), shape(1, 48_000));

    let mut out = vec![0.0; 2];
    let filled = converter.fill(&mut out, &mut src, 1.0);

    assert_eq!(filled.samples, 1, "one stereo frame is one mono frame");
    assert_eq!(bits(&out[..1]), bits(&[0.5]));
}

#[test]
fn a_wider_device_leaves_the_channels_the_source_has_no_answer_for_silent() {
    let mut src = TestSource::new(vec![0.5, 0.25], 2, 48_000);
    let mut converter = Converter::new(shape(2, 48_000), shape(4, 48_000));

    let mut out = vec![0.0; 4];
    let filled = converter.fill(&mut out, &mut src, 1.0);

    assert_eq!(filled.samples, 4);
    assert_eq!(bits(&out), bits(&[0.5, 0.25, 0.0, 0.0]));
}

/// A partial trailing frame would flip the deck's channel parity for whatever plays next, which is
/// the same hazard the ring's whole-frame pop and `EqSource`'s frame gate exist for.
#[test]
fn a_trailing_partial_frame_is_dropped_rather_than_padded() {
    let mut src = TestSource::new(vec![0.5, 0.25, 0.125], 2, 48_000);
    let mut converter = Converter::new(shape(2, 48_000), shape(2, 48_000));

    let mut out = vec![0.0; 8];
    let filled = converter.fill(&mut out, &mut src, 1.0);

    assert_eq!(filled.samples, 2, "the lone third sample is not half a frame of output");
    assert_eq!(filled.source_frames, 1);
}

/// The interpolation window is held across calls, so a short block is not a source boundary. Split
/// the same source two ways and the output has to be identical.
#[test]
fn a_block_boundary_is_not_a_source_boundary() {
    let (whole, _) = drain(ramp(64), 1, 44_100, 48_000, 1.0);

    let mut src = TestSource::new(ramp(64), 1, 44_100);
    let mut converter = Converter::new(shape(1, 44_100), shape(1, 48_000));
    let mut pieced = Vec::new();
    let mut block = vec![0.0; 7];
    while !converter.is_done() {
        let filled = converter.fill(&mut block, &mut src, 1.0);
        pieced.extend_from_slice(&block[..filled.samples]);
    }

    assert_eq!(bits(&pieced), bits(&whole));
}

#[test]
fn a_drained_converter_stays_drained() {
    let mut src = TestSource::new(ramp(2), 1, 44_100);
    let mut converter = Converter::new(shape(1, 44_100), shape(1, 44_100));

    let mut out = vec![0.0; 16];
    let first = converter.fill(&mut out, &mut src, 1.0);
    assert_eq!(first.samples, 2);
    assert!(converter.is_done());

    let second = converter.fill(&mut out, &mut src, 1.0);
    assert_eq!(
        second,
        Filled {
            samples: 0,
            source_frames: 0
        }
    );
}
