//! The safety limiter's release, which has to hand back exactly the level it took: anything short
//! of unity leaves the rest of the track a hair low and no longer bit-identical where it rests.

use super::Limiter;
use crate::player::playback::dsp::index_to_f32;

/// Far past the knee, so a held catch takes the gain down by more than 6 dB.
const LOUD_PEAK: f32 = 2.0;

/// Under the knee, where the limiter wants unity back.
const QUIET_PEAK: f32 = 0.25;

/// A catch of 1/20 s, ten attack time constants.
const CATCH_DIVISOR: usize = 20;

/// Past the 1.7 s the release from this catch takes to come within half an `f32` step of unity.
const RELEASE_SECONDS: usize = 2;

/// The gain from the last of `frames` frames peaking at `peak`.
fn hold(limiter: &mut Limiter, peak: f32, frames: usize) -> f32 {
    (0..frames).map(|_| limiter.process(peak)).last().unwrap_or(f32::NAN)
}

/// How far short of unity an `f32` release would settle grows with the rate, so each rate the
/// outputs run at is a row.
#[test]
fn after_a_catch_the_gain_returns_exactly_to_unity() {
    let cases = [44_100, 48_000, 96_000, 192_000];
    for rate in cases {
        let mut limiter = Limiter::new(index_to_f32(rate));
        let caught = hold(&mut limiter, LOUD_PEAK, rate / CATCH_DIVISOR);
        assert!(caught < 0.5, "at {rate} Hz the catch has to bite for this to mean anything");

        let released = hold(&mut limiter, QUIET_PEAK, RELEASE_SECONDS * rate);

        assert_eq!(
            released.to_bits(),
            1.0_f32.to_bits(),
            "at {rate} Hz the gain settled at {released}"
        );
    }
}
