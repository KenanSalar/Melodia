//! Tests for the shared decode primitives.
//!
//! [`super::ticks_to_frames`] is the one with two callers wanting opposite answers — `aac_trim`
//! trims the head and must never overshoot into real audio, `file_decode` trims after a seek and
//! must never undershoot into the previous packet's tail — so the rounding is the whole subject.

use symphonia::core::units::TimeBase;

use super::{Rounding, ticks_to_frames};
use crate::player::audio::SampleRate;

/// `ticks` against a `1/denom` timebase decoded at `rate`, the shape an MP4 audio track carries.
fn frames(ticks: u64, denom: u32, rate: u32, rounding: Rounding) -> Option<u64> {
    ticks_to_frames(ticks, TimeBase::try_new(1, denom)?, SampleRate::new(rate)?, rounding)
}

/// The overwhelmingly common case: the container counts in the decoder's own sample rate, so a
/// tick *is* a frame and neither rounding has anything to do.
#[test]
fn a_timescale_matching_the_decoder_converts_one_for_one() {
    for ticks in [0, 1, 1_024, 2_112, 45_124] {
        assert_eq!(frames(ticks, 44_100, 44_100, Rounding::Down), Some(ticks));
        assert_eq!(frames(ticks, 44_100, 44_100, Rounding::Up), Some(ticks));
    }
}

/// The case the conversion exists for: `aac_config` rewrites an HE-AAC config to its LC core, so
/// the decoder runs at half the rate the container declares while the edit list is still written
/// against the declared one.
#[test]
fn a_decoder_running_at_half_the_container_rate_halves_the_count() {
    assert_eq!(frames(2_112, 44_100, 22_050, Rounding::Down), Some(1_056));
    // An odd count is where the two roundings part company.
    assert_eq!(frames(2_113, 44_100, 22_050, Rounding::Down), Some(1_056));
    assert_eq!(frames(2_113, 44_100, 22_050, Rounding::Up), Some(1_057));
}

/// Where the ratio divides exactly there is nothing to round, so the two answers agree — which is
/// what makes the disagreement above the whole of the difference.
#[test]
fn the_two_roundings_agree_wherever_the_conversion_is_exact() {
    for (ticks, denom, rate) in [
        (1_000, 1_000, 48_000),
        (441, 44_100, 44_100),
        (0, 1_000, 44_100),
    ] {
        assert_eq!(
            frames(ticks, denom, rate, Rounding::Down),
            frames(ticks, denom, rate, Rounding::Up),
            "{ticks} ticks at {denom} → {rate} divides exactly and must round the same way"
        );
    }
}

/// ffmpeg's pairing: a movie timescale of 1000 against a 44 100 media rate, where almost nothing
/// divides. `Down` is `aac_trim`'s — a head that overshoots cuts the first frame of real audio off.
#[test]
fn a_millisecond_timescale_rounds_the_way_its_caller_asked() {
    // 23 ms at 44.1 kHz is 1014.3 frames.
    assert_eq!(frames(23, 1_000, 44_100, Rounding::Down), Some(1_014));
    assert_eq!(frames(23, 1_000, 44_100, Rounding::Up), Some(1_015));
}

/// The intermediate is a *product* of three numbers where the quotient is one, so it overflows
/// `u64` long before the answer does — which is why the arithmetic widens to `u128` first.
///
/// A `u64` intermediate would wrap and hand back a plausible small number rather than refusing,
/// and a trim built from it would cut somewhere arbitrary. The value is absurd on purpose: only a
/// corrupt container reaches here, and what has to hold is that it gets an arithmetically correct
/// answer rather than a wrapped one.
#[test]
fn a_product_that_would_overflow_a_u64_still_converts() {
    let ticks = 1_u64 << 50;
    // 2^50 × 48 000 is ~54e18, comfortably past u64::MAX — while the quotient is `ticks` itself.
    assert!(u64::try_from(u128::from(ticks) * 48_000).is_err(), "the premise stopped holding");

    assert_eq!(
        frames(ticks, 48_000, 48_000, Rounding::Down),
        Some(ticks),
        "the widening did not hold the intermediate"
    );
}

/// A result that genuinely cannot fit is a container stating nonsense, not a case to round — so
/// it is refused rather than saturated, and the caller falls back to trimming nothing.
#[test]
fn a_result_too_large_for_a_frame_count_is_refused() {
    assert_eq!(frames(u64::MAX, 1, 192_000, Rounding::Down), None);
    assert_eq!(frames(u64::MAX, 1, 192_000, Rounding::Up), None);
}

/// Zero ticks is the steady state — a file stating no priming and no edit — so it has to answer
/// `Some(0)` rather than `None`, which the callers read as "state nothing".
#[test]
fn a_zero_tick_count_converts_to_no_frames() {
    assert_eq!(frames(0, 1_000, 44_100, Rounding::Down), Some(0));
    assert_eq!(frames(0, 1_000, 44_100, Rounding::Up), Some(0));
}
