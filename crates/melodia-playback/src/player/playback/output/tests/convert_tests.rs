//! Tests for the rate and channel converter.
//!
//! The frame counts are exact rather than approximate, because the converter is what decides how
//! long a track is: a ratio that rounds the wrong way drifts against the clock counting the frames
//! it consumed, and the two are read against each other by every crossfade.

use melodia_audio::player::source::audio::Shape;

use std::f32::consts::TAU;

use super::{Converter, Filled};
use crate::player::playback::dsp::index_to_f32;
use crate::player::playback::tests::helpers::{TestSource, bits, shape};

/// The amplitude the level tests convert a tone at.
const TONE_AMPLITUDE: f32 = 0.5;

/// Source frames each level test converts, long enough that what the edges trim leaves thousands.
const TONE_FRAMES: usize = 19_200;

/// Output frames trimmed off each end before a level is read, past the kernel's reach at its
/// widest, where the source's start and end are still in the window.
const SETTLE_FRAMES: usize = 1_024;

/// How far below the tone fed in one past the output's Nyquist has to land. The kernel measured
/// about 140 dB down at both rows, so this leaves room for the f32 maths without letting an alias
/// through.
const STOPBAND_DB: f64 = -120.0;

/// How close to its own level a tone inside the band has to come back.
const PASSBAND_TOLERANCE_DB: f64 = 0.01;

/// `count` distinct, exactly representable samples, so an output can be traced back to its input.
fn ramp(count: usize) -> Vec<f32> {
    (0..count).map(|i| f32::from(u16::try_from(i).unwrap_or(u16::MAX)) / 1024.0).collect()
}

/// Fill `block` samples at a time until the converter is done, draining it once `src` runs dry, as
/// a voice does with nothing staged behind the source.
fn run_out(
    converter: &mut Converter,
    src: &mut TestSource,
    device: Shape,
    speed: f64,
    block: usize,
) -> (Vec<f32>, u64) {
    let mut out = Vec::new();
    let mut piece = vec![0.0; block];
    let mut frames = 0;
    while !converter.is_done() {
        let Filled { samples, source_frames } = converter.fill(&mut piece, src, device, speed);
        out.extend_from_slice(&piece[..samples]);
        frames += source_frames;
        if converter.is_starved() {
            converter.drain();
        }
    }
    (out, frames)
}

/// Convert a whole source from `from` to `to` at `speed`.
fn convert_all(source: Vec<f32>, channels: u16, from: u32, to: u32, speed: f64) -> (Vec<f32>, u64) {
    let mut src = TestSource::new(source, channels, from);
    let mut converter = Converter::new(shape(channels, from));
    run_out(&mut converter, &mut src, shape(channels, to), speed, 4096)
}

/// [`TONE_FRAMES`] of a `tone_hz` sine at `rate`, its phase kept inside one cycle in whole numbers.
///
/// A phase left to grow in f32 loses its low bits to rounding, and that noise lands inside the
/// band, where no converter stops it and it reads as leakage.
fn clean_tone(tone_hz: u32, rate: u32) -> Vec<f32> {
    let index = |n: u64| index_to_f32(usize::try_from(n).unwrap_or(usize::MAX));
    (0..TONE_FRAMES)
        .map(|frame| {
            let frame = u64::try_from(frame).unwrap_or(u64::MAX);
            let into_cycle = frame * u64::from(tone_hz) % u64::from(rate);
            TONE_AMPLITUDE * (TAU * index(into_cycle) / index(u64::from(rate))).sin()
        })
        .collect()
}

/// The level of a `tone_hz` sine after converting it from `from` to `to` Hz at `speed`, in dB
/// against the level it went in at, read away from both ends.
fn level_after_conversion(tone_hz: u32, from: u32, to: u32, speed: f64) -> f64 {
    let (out, _) = convert_all(clean_tone(tone_hz, from), 1, from, to, speed);
    let end = out.len().saturating_sub(SETTLE_FRAMES);
    let settled = out.get(SETTLE_FRAMES..end).unwrap_or_default();
    let power = settled.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>()
        / f64::from(index_to_f32(settled.len()));
    let tone_power = f64::from(TONE_AMPLITUDE).powi(2) / 2.0;
    10.0 * (power / tone_power).log10()
}

/// Convert `data` from `source` channels to `device` channels, at one rate.
fn remap(data: Vec<f32>, source: u16, device: u16) -> (Vec<f32>, u64) {
    let mut src = TestSource::new(data, source, 48_000);
    let mut converter = Converter::new(shape(source, 48_000));
    run_out(&mut converter, &mut src, shape(device, 48_000), 1.0, 4096)
}

#[test]
fn equal_rates_pass_every_sample_through_untouched() {
    let input = ramp(8);
    let (out, frames) = convert_all(input.clone(), 1, 44_100, 44_100, 1.0);

    assert_eq!(bits(&out), bits(&input), "an equal-rate fill must not touch a single sample");
    assert_eq!(frames, 8);
}

/// The passthrough has to survive the sign of zero, which is what a multiply-add loses: `-0.0` is
/// the sample a silent stretch of a signed format decodes to, and bit-perfect means bit-perfect.
#[test]
fn the_passthrough_keeps_negative_zero() {
    let (out, _) = convert_all(vec![-0.0, 0.0, -0.0], 1, 48_000, 48_000, 1.0);
    assert_eq!(bits(&out), bits(&[-0.0, 0.0, -0.0]));
}

#[test]
fn doubling_the_rate_doubles_the_frames() {
    let (out, frames) = convert_all(ramp(4), 1, 24_000, 48_000, 1.0);
    assert_eq!(out.len(), 8);
    assert_eq!(frames, 4);
}

#[test]
fn halving_the_rate_halves_the_frames() {
    let (out, frames) = convert_all(ramp(8), 1, 48_000, 24_000, 1.0);
    assert_eq!(out.len(), 4);
    assert_eq!(frames, 8);
}

/// The ratio every library hits and no device offers: 44.1 kHz material on a 48 kHz device.
#[test]
fn the_common_ratio_lands_within_a_frame_of_its_own_arithmetic() {
    let (out, frames) = convert_all(ramp(441), 1, 44_100, 48_000, 1.0);
    assert_eq!(frames, 441);
    let want = 441 * 48_000 / 44_100;
    assert!(out.len().abs_diff(want) <= 1, "got {} output frames, expected ~{want}", out.len());
}

/// Playback speed is the same ratio from the other side, so it must arrive at the same count.
#[test]
fn speed_scales_the_ratio_rather_than_the_reported_rate() {
    let (half, _) = convert_all(ramp(8), 1, 48_000, 48_000, 2.0);
    let (double, _) = convert_all(ramp(4), 1, 48_000, 48_000, 0.5);
    assert_eq!(half.len(), 4, "double speed consumes two source frames per output frame");
    assert_eq!(double.len(), 8, "half speed emits two output frames per source frame");
}

/// Dropping the final frame is a click at every gapless boundary, which is the one place a listener
/// would hear it — the tracks either side are meant to be continuous.
#[test]
fn the_last_source_frame_is_handed_over() {
    let input = ramp(5);
    let (out, _) = convert_all(input.clone(), 1, 44_100, 44_100, 1.0);
    assert_eq!(out.last().map(|s| s.to_bits()), input.last().map(|s| s.to_bits()));
}

#[test]
fn a_one_frame_source_yields_that_frame() {
    let (out, frames) = convert_all(vec![0.25], 1, 44_100, 44_100, 1.0);
    assert_eq!(bits(&out), bits(&[0.25]));
    assert_eq!(frames, 1);
}

#[test]
fn an_empty_source_yields_nothing() {
    let (out, frames) = convert_all(Vec::new(), 1, 44_100, 44_100, 1.0);
    assert!(out.is_empty());
    assert_eq!(frames, 0);
}

/// Mono into a stereo device is the case the mapping exists for: silence in the right channel is
/// what a naive copy gives, and it is what a listener reports as "it only plays on one side".
#[test]
fn a_mono_source_is_duplicated_across_a_stereo_device() {
    let (out, _) = remap(vec![0.5, 0.25], 1, 2);
    assert_eq!(bits(&out), bits(&[0.5, 0.5, 0.25, 0.25]));
}

/// A mono device is the config ladder's second rung, so this is the fold that has to be right:
/// keeping channel 0 alone would play the left half of every stereo file.
#[test]
fn a_mono_device_is_handed_the_average_rather_than_the_left_channel() {
    let (out, _) = remap(vec![0.5, 0.25], 2, 1);

    assert_eq!(out.len(), 1, "one stereo frame is one mono frame");
    // (0.5 + 0.25) / 2 is exact in binary, so the mean can be pinned bit-for-bit like its neighbours.
    assert_eq!(bits(&out), bits(&[0.375]), "the fold is the mean, not the left channel");
}

/// Nothing above a mono device folds: `Shape` counts channels without naming them, so there is no
/// layout to fold along and a blind one would put LFE and surrounds into the mains at full scale.
#[test]
fn a_narrower_device_wider_than_mono_still_carries_the_channels_it_can() {
    let (out, _) = remap(vec![0.5, 0.25, 0.125, 0.0625], 4, 2);
    assert_eq!(bits(&out), bits(&[0.5, 0.25]));
}

/// The path every mono file on an ordinary stereo device takes — so each channel has to stay the
/// source's own sample, with no pan-law trim between the decoder and the card.
#[test]
fn mono_reaches_a_stereo_device_at_unity_on_both_channels() {
    let (out, _) = remap(vec![0.5, -0.25], 1, 2);
    assert_eq!(bits(&out), bits(&[0.5, 0.5, -0.25, -0.25]));
}

#[test]
fn a_wider_device_leaves_the_channels_the_source_has_no_answer_for_silent() {
    let (out, _) = remap(vec![0.5, 0.25], 2, 4);
    assert_eq!(bits(&out), bits(&[0.5, 0.25, 0.0, 0.0]));
}

/// A partial trailing frame would flip the voice's channel parity for whatever plays on it next,
/// which is the same hazard the ring's whole-frame pop and `EqSource`'s frame gate exist for.
#[test]
fn a_trailing_partial_frame_is_dropped_rather_than_padded() {
    let (out, frames) = remap(vec![0.5, 0.25, 0.125], 2, 2);

    assert_eq!(out.len(), 2, "the lone third sample is not half a frame of output");
    assert_eq!(frames, 1);
}

/// The interpolation window is held across calls, so a short block is not a source boundary. Split
/// the same source two ways and the output has to be identical.
#[test]
fn a_block_boundary_is_not_a_source_boundary() {
    let (whole, _) = convert_all(ramp(64), 1, 44_100, 48_000, 1.0);

    let mut src = TestSource::new(ramp(64), 1, 44_100);
    let mut converter = Converter::new(shape(1, 44_100));
    let (pieced, _) = run_out(&mut converter, &mut src, shape(1, 48_000), 1.0, 7);

    assert_eq!(bits(&pieced), bits(&whole));
}

#[test]
fn a_drained_converter_stays_drained() {
    let mut src = TestSource::new(ramp(2), 1, 44_100);
    let device = shape(1, 44_100);
    let mut converter = Converter::new(device);
    let (out, _) = run_out(&mut converter, &mut src, device, 1.0, 16);
    assert_eq!(out.len(), 2);

    let again = converter.fill(&mut [0.0; 16], &mut src, device, 1.0);
    assert_eq!(again, Filled { samples: 0, source_frames: 0 });
}

/// The length a track plays for is decided here, so the count is pinned exactly: an output frame
/// for every output instant inside the source's span, which rounds a fractional count up. The
/// ratios are ones whose count falls well clear of a whole number, or whose step is exact in
/// binary, so the rounding asked about is the converter's and not the accumulated position's.
#[test]
fn a_drained_source_yields_an_output_frame_for_every_instant_inside_it() {
    let rows: [(usize, u32, u32, usize); 5] = [
        (1_000, 44_100, 48_000, 1_089),
        (1_000, 48_000, 44_100, 919),
        (999, 96_000, 48_000, 500),
        (1_000, 96_000, 48_000, 500),
        (1_001, 24_000, 48_000, 2_002),
    ];
    for (frames, from, to, expected) in rows {
        let (out, consumed) = convert_all(ramp(frames), 1, from, to, 1.0);

        assert_eq!(out.len(), expected, "{frames} frames from {from} to {to} Hz");
        assert_eq!(usize::try_from(consumed).ok(), Some(frames), "every source frame is taken");
    }
}

/// Downsampling, or playing faster than the source, puts tones the source carries past the
/// output's Nyquist, and the kernel has to stop them rather than fold them back into the band.
#[test]
fn a_tone_past_the_outputs_nyquist_is_stopped() {
    let rows = [
        ("96 to 48 kHz, a 30 kHz tone", 96_000, 48_000, 1.0, 30_000),
        ("48 kHz at twice the speed, a 15 kHz tone", 48_000, 48_000, 2.0, 15_000),
    ];
    for (what, from, to, speed, tone_hz) in rows {
        let level = level_after_conversion(tone_hz, from, to, speed);

        assert!(level < STOPBAND_DB, "{what} came through at {level:.1} dB");
    }
}

/// The stopband's other side: a converter that stopped everything would pass the test above.
#[test]
fn a_tone_inside_the_band_keeps_its_level() {
    let rows = [
        ("44.1 to 48 kHz, a 10 kHz tone", 44_100, 48_000, 1.0, 10_000),
        ("96 to 48 kHz, a 10 kHz tone", 96_000, 48_000, 1.0, 10_000),
        ("48 kHz at twice the speed, a 5 kHz tone", 48_000, 48_000, 2.0, 5_000),
    ];
    for (what, from, to, speed, tone_hz) in rows {
        let level = level_after_conversion(tone_hz, from, to, speed);

        assert!(level.abs() < PASSBAND_TOLERANCE_DB, "{what} came through at {level:.4} dB");
    }
}

/// How far a step between two interpolated frames may stray from the speed that took it. The
/// kernel reads a ramp as a ramp to far better than this; the slack is for the f32 position.
const STEP_TOLERANCE: f32 = 0.01;

/// Speeds a live change walks through, one block each. None is below one, where returning to the
/// passthrough drops a fraction of a frame and can step back by that much by design.
const LIVE_SPEEDS: [f64; 5] = [1.0, 1.25, 1.0, 1.5, 1.0];

/// The fastest of [`LIVE_SPEEDS`], as the step it takes.
const FASTEST_STEP: f32 = 1.5;

/// How far apart each frame written while [`LIVE_SPEEDS`] change between blocks reads, in source
/// frames: the source counts its own frames, so a written value is the position it was read at.
fn steps_across_live_speed_changes() -> Vec<f32> {
    let device = shape(1, 48_000);
    let positions = (0..6_000).map(index_to_f32).collect();
    let mut src = TestSource::new(positions, 1, 48_000);
    let mut converter = Converter::new(device);
    let mut written = Vec::new();
    for speed in LIVE_SPEEDS {
        let mut block = vec![0.0; 800];
        let Filled { samples, .. } = converter.fill(&mut block, &mut src, device, speed);
        written.extend_from_slice(&block[..samples]);
    }
    written.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

/// A replayed frame reads as a step of zero, or back.
#[test]
fn a_live_speed_change_never_replays_a_frame() {
    let steps = steps_across_live_speed_changes();

    let shortest = steps.iter().copied().fold(f32::INFINITY, f32::min);

    assert!(shortest > 0.0, "the read position stood still or went back, by {shortest} frames");
}

/// A dropped frame reads as a step one longer than the speed asks for.
#[test]
fn a_live_speed_change_never_skips_a_frame() {
    let steps = steps_across_live_speed_changes();

    let longest = steps.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    assert!(longest <= FASTEST_STEP + STEP_TOLERANCE, "the read position jumped {longest} frames");
}
