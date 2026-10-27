//! Primitives the DSP paths share: the state cells on the audio thread, and the visualizer's
//! analysis on the UI thread.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// What a falling bar or trace keeps per frame, shared so every style falls by one law. The settle
/// in wall time is this against the strip's one Timer interval (`visualizer-strip.slint`).
pub(crate) const VISUALIZER_DECAY: f32 = 0.8;

// Shrinking every frame without inverting or vanishing, or the smoother snaps.
const _: () = assert!(
    VISUALIZER_DECAY > 0.0 && VISUALIZER_DECAY < 1.0,
    "the visualizer decay must shrink a level without flipping its sign"
);

#[expect(
    clippy::cast_precision_loss,
    reason = "callers pass window, bin and column indices, which are counts in the low thousands"
)]
pub(crate) fn index_to_f32(i: usize) -> f32 {
    i as f32
}

/// `value` with exactly `DECIMALS` places through integer maths, not `core::fmt`'s exactly-rounded
/// decimal twice per vertex per frame. A `DECIMALS` too wide for a `u16` fails at codegen, which
/// clippy passes. Unlike `core::fmt`, negative zero prints `0` and a tie rounds away from zero.
pub(crate) fn push_fixed<const DECIMALS: u32>(out: &mut String, value: f32) {
    let scale = const { 10u16.pow(DECIMALS) };

    #[expect(
        clippy::cast_possible_truncation,
        reason = "coordinates are normalized to a unit range, so the scaled value is at most ±10_000; a float→int `as` saturates besides, so even a forged input clamps rather than wrapping"
    )]
    let units = (value * f32::from(scale)).round() as i32;

    if units < 0 {
        out.push('-');
    }
    let magnitude = units.unsigned_abs();
    let scale = u32::from(scale);
    let width = DECIMALS as usize;
    // Writing into a String cannot fail.
    let _ = write!(out, "{}.{:0width$}", magnitude / scale, magnitude % scale);
}

pub(crate) fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Silence has no decibel value, so both callers guard the domain first.
pub(crate) fn linear_to_db(lin: f32) -> f32 {
    20.0 * lin.log10()
}

/// A change counter for state shared with the audio thread: the control side writes a cell's
/// `Relaxed` fields and bumps it, and the source polls it each frame, the `Release`/`Acquire` pair
/// publishing those writes. Starts at 1, so a source seeding `0` rebuilds before its first sample,
/// which a gapless successor on an armed deck relies on.
pub(crate) struct Generation(AtomicU64);

impl Generation {
    pub(crate) fn new() -> Self {
        Self(AtomicU64::new(1))
    }

    pub(crate) fn bump(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

/// A float in an atomic, `Relaxed` both ways: a [`Generation`] reader takes its ordering there, and
/// a deck's volume or speed read stale costs one block at the previous value.
macro_rules! atomic_float {
    ($name:ident, $float:ty, $cell:ty) => {
        pub(crate) struct $name($cell);

        impl $name {
            pub(crate) fn new(value: $float) -> Self {
                Self(<$cell>::new(value.to_bits()))
            }

            pub(crate) fn store(&self, value: $float) {
                self.0.store(value.to_bits(), Ordering::Relaxed);
            }

            pub(crate) fn load(&self) -> $float {
                <$float>::from_bits(self.0.load(Ordering::Relaxed))
            }
        }
    };
}

atomic_float!(AtomicF32, f32, AtomicU32);
atomic_float!(AtomicF64, f64, AtomicU64);

#[cfg(test)]
#[path = "tests/dsp_tests.rs"]
mod tests;
