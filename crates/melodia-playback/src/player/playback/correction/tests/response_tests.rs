//! The headroom a correction takes. Too little clips a boost into the limiter; too much plays the
//! whole track quieter than it has to, and at a high rate a few ulps decide which.
//!
//! The expected values are the cookbook's: a peaking filter's gain at its centre is exactly the
//! gain it was designed with, so the headroom over one peak is that gain's inverse.

use biquad::Type;

use super::{FilterResponse, ResponseGrid};
use crate::player::playback::dsp;

const RATE: f64 = 48_000.0;

/// `10^(-6/20)` and `10^(-12/20)`.
const SIX_DB_DOWN: f64 = 0.501_187_233_627_272_3;
const TWELVE_DB_DOWN: f64 = 0.251_188_643_150_958;

/// `10^(-30/20)`, the largest boost a correction filter may hold.
const THIRTY_DB_DOWN: f64 = 0.031_622_776_601_683_79;

/// Well under any level a listener could hear, and far above `f64` rounding at a mid rate.
const TOLERANCE: f64 = 1e-9;

/// Looser than [`TOLERANCE`]: a sub-bass filter at a high rate is the few ulps left after the
/// cookbook's terms cancel.
const SUB_BASS_TOLERANCE: f64 = 4e-6;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn peak(
    grid: &ResponseGrid,
    rate: f64,
    centre_hz: f64,
    gain_db: f64,
    q: f64,
) -> Option<FilterResponse> {
    let coeffs = dsp::design(Type::PeakingEQ(gain_db), rate, centre_hz, q)?;
    Some(grid.response(&coeffs, centre_hz))
}

const NO_DESIGN: &str = "the cookbook refused a filter inside the source's band";

fn assert_near(actual: f64, expected: f64, tolerance: f64, what: &str) {
    assert!((actual - expected).abs() < tolerance, "{what}: expected {expected}, got {actual}");
}

#[test]
fn a_boost_comes_down_by_its_peak() -> TestResult {
    let grid = ResponseGrid::new(RATE);
    let boost = peak(&grid, RATE, 1_000.0, 6.0, 1.41).ok_or(NO_DESIGN)?;

    let headroom = grid.headroom(&[boost], &[]);

    assert_near(headroom, SIX_DB_DOWN, TOLERANCE, "+6 dB at 1 kHz");
    Ok(())
}

/// The level only comes down, so a correction that only cuts leaves it where it was.
#[test]
fn a_cut_takes_no_headroom() -> TestResult {
    let grid = ResponseGrid::new(RATE);
    let cut = peak(&grid, RATE, 1_000.0, -6.0, 1.41).ok_or(NO_DESIGN)?;

    let headroom = grid.headroom(&[cut], &[]);

    assert_near(headroom, 1.0, TOLERANCE, "-6 dB at 1 kHz");
    Ok(())
}

/// A slider boosting where the correction already does stacks on it, and the headroom has to take
/// both, or the sum reaches the limiter.
#[test]
fn the_bands_count_with_the_correction() -> TestResult {
    let grid = ResponseGrid::new(RATE);
    let correction = peak(&grid, RATE, 1_000.0, 6.0, 1.41).ok_or(NO_DESIGN)?;
    let band = peak(&grid, RATE, 1_000.0, 6.0, 1.41).ok_or(NO_DESIGN)?;

    let headroom = grid.headroom(&[correction], &[band]);

    assert_near(headroom, TWELVE_DB_DOWN, TOLERANCE, "+6 dB twice at 1 kHz");
    Ok(())
}

/// The narrowest, largest boost the range allows, at the grid's lowest point: its poles sit
/// nearest z = 1, and nearer still as the rate climbs.
#[test]
fn a_sub_bass_peak_keeps_its_headroom_at_every_rate() -> TestResult {
    for rate in [44_100.0, 96_000.0, 192_000.0, 352_800.0, 768_000.0] {
        let grid = ResponseGrid::new(rate);
        let boost = peak(&grid, rate, 20.0, 30.0, 20.0).ok_or(NO_DESIGN)?;

        let headroom = grid.headroom(&[boost], &[]);

        assert_near(headroom, THIRTY_DB_DOWN, SUB_BASS_TOLERANCE, &format!("{rate} Hz"));
    }
    Ok(())
}
