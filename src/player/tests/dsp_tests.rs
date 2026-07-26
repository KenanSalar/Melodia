//! Tests for the shared DSP primitives.

use super::{Generation, db_to_linear, index_to_f32, linear_to_db};
use crate::player::tests::helpers::approx_eq as approx;

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
