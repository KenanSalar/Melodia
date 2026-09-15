//! Tiny primitives shared across the player's DSP paths — the equalizer,
//! `ReplayGain` and crossfade state cells on the audio thread, and the
//! visualizer's spectrum and waveform analysis on the UI thread.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Fraction of its height a visualizer bar or trace keeps per frame while falling.
/// Shared so the two styles fall by the same law rather than each carrying its own —
/// a decay is a decay whichever drawing is showing.
///
/// Per *frame*, so the wall-clock settle is this against the strip's Timer, and that
/// is one interval for every style (`visualizer-strip.slint`). Retuning how fast a
/// drawing dies away means reaching for one of those two numbers; a per-style rate
/// would move all three settle times off this constant at once.
pub(crate) const VISUALIZER_DECAY: f32 = 0.8;

// A bar or trace has to lose height every frame but never invert or vanish
// outright, or the smoother snaps instead of settling.
const _: () = assert!(
    VISUALIZER_DECAY > 0.0 && VISUALIZER_DECAY < 1.0,
    "the visualizer decay must shrink a level without flipping its sign"
);

/// Widen a count to `f32`. Every caller passes a window, bin or column index — counts
/// in the low thousands at most, which `f32` represents exactly.
#[expect(
    clippy::cast_precision_loss,
    reason = "callers pass window, bin and column indices, which are counts in the low thousands"
)]
pub(crate) fn index_to_f32(i: usize) -> f32 {
    i as f32
}

/// Append `value` to `out` with exactly `DECIMALS` fractional digits.
///
/// Both drawn styles serialize their figure as SVG path text every tick, and their coordinates are
/// normalized and quantized to a few places, so the format is a sign, one digit and a zero-padded
/// remainder. `{value:.N$}` would instead ask `core::fmt` for the exactly-rounded decimal, twice
/// per vertex per frame.
///
/// `DECIMALS` is a const parameter so a request too wide for a `u16` overflows the `const` block
/// rather than needing a runtime clamp — but that diagnostic arrives at **codegen**, so `cargo
/// clippy`, this repo's usual gate, passes what `cargo build` rejects.
///
/// Two deliberate differences from `core::fmt`, each worth one unit in the last place on a unit
/// viewbox: negative zero prints as `0`, and an exact tie rounds away from zero rather than to
/// even.
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

/// Convert a decibel value to a linear amplitude factor.
pub(crate) fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Convert a linear amplitude factor to decibels — the inverse of [`db_to_linear`].
///
/// Silence has no decibel value, so callers guard the domain first. Both already do
/// for their own reasons: the limiter returns unity below its knee, and the spectrum
/// analyzer floors quiet bins at zero.
pub(crate) fn linear_to_db(lin: f32) -> f32 {
    20.0 * lin.log10()
}

/// A lock-free change counter for state shared with the audio thread.
///
/// The control side mutates a cell's `Relaxed` fields and then [`bump`](Self::bump)s
/// this; the audio source caches the value it last acted on and polls
/// [`get`](Self::get) each frame, recomputing only when the two differ. The
/// `Release`/`Acquire` pair is what publishes those field writes — they precede the
/// bump, so a reader observing the new generation observes them too.
///
/// Starts at **1**, so a source seeding its cached value to `0` is guaranteed to
/// rebuild before its first sample — the hook a gapless successor appended to an
/// already-armed deck relies on.
///
/// `EqShared`, `ReplayGainShared` and `FadeShared` each hold one. `CrossfadeShared`
/// deliberately does not, being read by the control layer only.
pub(crate) struct Generation(AtomicU64);

impl Generation {
    pub(crate) fn new() -> Self {
        Self(AtomicU64::new(1))
    }

    /// Publish every field write that precedes this call.
    pub(crate) fn bump(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    /// The current generation. Pairs with the `Release` in [`Self::bump`].
    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

/// A float in an atomic, which std does not offer.
///
/// **`Relaxed` on both halves, and that is the cell's whole ordering story.** Two kinds of reader
/// share these: one polls a [`Generation`] and gets its ordering from the `Release`/`Acquire` pair
/// there, so a stricter load here would only pay for a guarantee it already has; the other — a
/// deck's volume and speed — is the audio callback reading a level nobody publishes alongside
/// anything, where the worst a stale read costs is one block at the previous value.
///
/// Written out four times before this existed, each site re-deriving the bit-pattern round trip and
/// picking an ordering of its own.
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
