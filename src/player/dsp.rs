//! Tiny primitives shared across the player's DSP paths — the equalizer,
//! `ReplayGain` and crossfade state cells on the audio thread, and the
//! visualizer's spectrum and waveform analysis on the UI thread.

use std::sync::atomic::{AtomicU64, Ordering};

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

#[cfg(test)]
#[path = "tests/dsp_tests.rs"]
mod tests;
