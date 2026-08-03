//! Tiny primitives shared across the player's DSP paths — the equalizer,
//! `ReplayGain` and crossfade state cells on the audio thread, and the
//! visualizer's spectrum and waveform analysis on the UI thread.

use std::sync::atomic::{AtomicU64, Ordering};

/// Fraction of its height a visualizer bar or trace keeps per frame while
/// falling. Shared so the two styles fall by the same law rather than each
/// carrying its own — a decay is a decay whichever drawing is showing.
///
/// Per *frame*, so the wall-clock settle is this against the strip's Timer —
/// one interval for every style (`visualizer-strip.slint`: 33 ms, or 500 ms once
/// dormant), so all three settle in about a second. Retuning how fast a drawing
/// dies away means reaching for one of those two numbers; there is no per-style
/// rate to reach for, and adding one would move all three styles' settle times
/// off this constant at once.
pub(crate) const VISUALIZER_DECAY: f32 = 0.8;

// A bar or trace has to lose height every frame but never invert or vanish
// outright, or the smoother snaps instead of settling.
const _: () = assert!(
    VISUALIZER_DECAY > 0.0 && VISUALIZER_DECAY < 1.0,
    "the visualizer decay must shrink a level without flipping its sign"
);

/// Widen a count to `f32`.
///
/// Every caller is a window, bin or column index — counts in the low thousands
/// at most, which `f32` represents exactly.
#[expect(
    clippy::cast_precision_loss,
    reason = "callers pass window, bin and column indices, which are counts in the low thousands"
)]
pub(crate) fn index_to_f32(i: usize) -> f32 {
    i as f32
}

/// Convert a decibel value to a linear amplitude factor.
pub(crate) fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Convert a linear amplitude factor to decibels — the inverse of
/// [`db_to_linear`].
///
/// Silence has no decibel value (`log10(0)` is `-inf`), so callers guard the
/// domain before calling. Both of them already do it for their own reasons: the
/// limiter returns unity below its knee, and the spectrum analyzer floors quiet
/// bins at zero.
pub(crate) fn linear_to_db(lin: f32) -> f32 {
    20.0 * lin.log10()
}

/// A lock-free change counter for state shared with the audio thread.
///
/// The control side mutates a cell's fields (plain `Relaxed` atomics) and then
/// [`bump`](Self::bump)s this; the audio source caches the value it last acted on
/// and polls [`get`](Self::get) each frame, recomputing only when the two differ.
/// The `Release`/`Acquire` pair is what publishes the field writes: they precede
/// the bump, so a reader that observes the new generation also observes them.
///
/// Starts at **1**, so a source can seed its cached value to `0` (or any
/// `wrapping_sub(1)`) and be guaranteed to rebuild before its first sample — the
/// hook a gapless successor appended to an already-armed deck relies on.
///
/// `EqShared`, `ReplayGainShared` and `FadeShared` each hold one. `CrossfadeShared`
/// deliberately does not: it is read by the control layer only, never the audio
/// thread, so there is nothing to poll.
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

#[cfg(test)]
#[path = "tests/dsp_tests.rs"]
mod tests;
