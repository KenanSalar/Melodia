//! Tests for the shared DSP primitives.

use std::time::Duration;

use super::{Generation, db_to_linear, index_to_f32, linear_to_db};
use crate::player::audio::{frames_in, frames_to_duration, interleaved};
use crate::player::tests::helpers::{approx_eq as approx, nz_u16 as channels, nz_u32 as rate};

#[test]
fn zero_decibels_is_unity_gain() {
    assert!(approx(db_to_linear(0.0), 1.0));
    assert!(approx(linear_to_db(1.0), 0.0));
}

#[test]
fn six_decibels_is_roughly_double() {
    assert!(approx(db_to_linear(6.0206), 2.0));
    assert!(approx(linear_to_db(2.0), 6.0206));
    // ...and its negation halves, which is the pair `ReplayGain` attenuates by.
    assert!(approx(db_to_linear(-6.0206), 0.5));
}

#[test]
fn a_negative_decibel_attenuates() {
    assert!(db_to_linear(-20.0) < 1.0);
    assert!(approx(db_to_linear(-20.0), 0.1));
    assert!(approx(db_to_linear(20.0), 10.0));
}

#[test]
fn the_two_conversions_round_trip() {
    for db in [-75.0, -40.0, -15.0, -6.0, 0.0, 6.0] {
        assert!(approx(linear_to_db(db_to_linear(db)), db));
    }
}

#[test]
fn a_count_widens_exactly() {
    // The FFT and waveform sizes the visualizer actually passes, plus the
    // edges — all well inside f32's exactly-representable integer range.
    for (count, expected) in [
        (0_usize, 0.0_f32),
        (1, 1.0),
        (64, 64.0),
        (2048, 2048.0),
        (8192, 8192.0),
        (16_384, 16_384.0),
    ] {
        assert!(approx(index_to_f32(count), expected));
    }
}

#[test]
fn a_generation_starts_at_one_so_a_fresh_source_rebuilds() {
    // A source seeds its cached value to 0 (or any `wrapping_sub(1)`) and is
    // then guaranteed to differ on its first poll.
    let generation = Generation::new();
    assert_eq!(generation.get(), 1);
    assert_ne!(generation.get(), 0);
}

#[test]
fn a_bump_publishes_a_new_value() {
    let generation = Generation::new();
    let before = generation.get();
    generation.bump();
    assert_ne!(generation.get(), before);
}

// ------------------------------------------------- frames, durations and channels

/// **Neither direction is an identity, and that is the contract rather than a defect**: both
/// halves floor, so a value that does not land on a frame boundary loses its remainder and a
/// round trip loses it twice. What has to hold is that the loss is bounded at one frame and only
/// ever *downward* — a conversion that could inflate would let a resumed deck seek past where it
/// stopped, and one that drifted by more than a frame would be audible at the handover.
///
/// The transport never leans on more than this: every `Duration` it produces is a whole number of
/// milliseconds, which at any of these rates is dozens of frames.
#[test]
fn a_round_trip_loses_at_most_one_frame_and_never_gains_one() {
    for hz in [8_000, 44_100, 48_000, 96_000, 192_000] {
        for frames in [0_u64, 1, 441, 44_099, 48_000, 1_234_567, 900_000_000] {
            let there_and_back = frames_in(frames_to_duration(frames, rate(hz)), rate(hz));
            assert!(
                there_and_back <= frames && frames - there_and_back <= 1,
                "{frames} frames at {hz}Hz came back as {there_and_back}"
            );
        }
    }
}

/// A duration converts *down*, never to nearest: the answer is the frames wholly contained in it.
/// An implementation that rounded would pass every other test here and let a seek land a frame
/// past what the slider asked for.
#[test]
fn a_duration_that_lands_between_frames_keeps_only_the_whole_ones() {
    let cd = rate(44_100);
    // 7 ms is 308.7 frames at 44.1 kHz — the .7 is dropped, not rounded up.
    assert_eq!(frames_in(Duration::from_millis(7), cd), 308);
    // And a frame boundary itself is not lost: 10 ms is exactly 441.
    assert_eq!(frames_in(Duration::from_millis(10), cd), 441);
}

/// **The property the two merged implementations agreed on**, and the reason merging them changed
/// no behaviour: every `Duration` the transport produces is a whole number of milliseconds, and
/// there the nanosecond form and the microsecond one it replaced compute the same frame count.
///
/// Spelled as the *old* arithmetic rather than as a table, so this keeps answering for a rate
/// nobody thought to tabulate.
#[test]
fn on_whole_milliseconds_it_agrees_with_the_microsecond_form_it_replaced() {
    for hz in [8_000, 44_100, 48_000, 96_000, 192_000] {
        for ms in [0_u64, 1, 7, 13, 500, 999, 1_000, 60_000, 3_723_004] {
            let span = Duration::from_millis(ms);
            let micros = u64::try_from(span.as_micros().saturating_mul(u128::from(hz)) / 1_000_000)
                .unwrap_or(u64::MAX);
            assert_eq!(
                frames_in(span, rate(hz)),
                micros,
                "{ms}ms at {hz}Hz: the two forms parted company"
            );
        }
    }
}

/// The rates that divide a second evenly would survive a microsecond conversion too. 44 100 is
/// the one that would not, which is why the pair counts nanoseconds.
#[test]
fn a_rate_that_does_not_divide_a_second_still_lands_on_the_frame() {
    let cd = rate(44_100);
    // One frame at 44.1 kHz is ~22.676 µs, so a microsecond-truncating conversion loses it.
    assert_eq!(frames_in(Duration::from_nanos(22_676), cd), 1);
    assert_eq!(frames_in(Duration::from_secs(1), cd), 44_100);
    assert_eq!(frames_to_duration(44_100, cd), Duration::from_secs(1));
}

#[test]
fn a_whole_second_is_exactly_the_rate_in_frames() {
    for hz in [8_000, 44_100, 48_000, 192_000] {
        assert_eq!(frames_in(Duration::from_secs(1), rate(hz)), u64::from(hz));
        assert_eq!(frames_in(Duration::ZERO, rate(hz)), 0);
    }
}

/// A length read back out of a container is bounded only by what fits a `u64`, and a corrupt one
/// states whatever it likes — so the widening saturates rather than wrapping.
#[test]
fn a_frame_count_a_container_invented_saturates_rather_than_wrapping() {
    assert_eq!(interleaved(u64::MAX, channels(2)), u64::MAX);
    assert_eq!(frames_in(Duration::from_secs(u64::MAX), rate(48_000)), u64::MAX);
}

#[test]
fn interleaving_multiplies_by_the_channel_count() {
    assert_eq!(interleaved(0, channels(2)), 0);
    assert_eq!(interleaved(1_024, channels(1)), 1_024);
    assert_eq!(interleaved(1_024, channels(2)), 2_048);
    assert_eq!(interleaved(1_024, channels(6)), 6_144);
}
