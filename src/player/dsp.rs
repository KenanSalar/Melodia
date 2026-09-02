//! Tiny primitives shared across the player's DSP paths — the equalizer,
//! `ReplayGain` and crossfade state cells on the audio thread, and the
//! visualizer's spectrum and waveform analysis on the UI thread.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use super::audio::{ChannelCount, SampleRate};

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

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Frames of a source running at `rate` that `span` is worth, rounded **down**.
///
/// Seconds and nanoseconds separately, so a rate that does not divide a second evenly cannot cost
/// the answer a frame the way a float round trip or a microsecond truncation would.
///
/// [`frames_to_duration`] is the way back but not an inverse: both floor, so a value off a frame
/// boundary loses its remainder and a round trip loses it twice. The bound is one frame, downward
/// — which is all the transport needs, every `Duration` it produces being a whole number of
/// milliseconds, and dozens of frames at any rate. `player::tests::dsp_tests` pins both halves.
pub(crate) fn frames_in(span: Duration, rate: SampleRate) -> u64 {
    let rate = u64::from(rate.get());
    let subsec = u64::from(span.subsec_nanos()) * rate / NANOS_PER_SEC;
    span.as_secs().saturating_mul(rate).saturating_add(subsec)
}

/// How long `frames` at `rate` play for — [`frames_in`]'s counterpart, with the same flooring.
pub(crate) fn frames_to_duration(frames: u64, rate: SampleRate) -> Duration {
    let rate = u64::from(rate.get());
    let nanos = (frames % rate) * NANOS_PER_SEC / rate;
    Duration::new(frames / rate, u32::try_from(nanos).unwrap_or(0))
}

/// `frames` as interleaved samples.
///
/// Saturating, because the only bound on a length read back out of a container is what fits a
/// `u64`, and a corrupt one states whatever it likes.
///
/// Here rather than beside its one caller because it is the third term of the same vocabulary as
/// [`frames_in`] and [`frames_to_duration`] — frames, the time they play for, and the samples
/// they occupy — and splitting the trio costs more than the single consumer saves.
pub(crate) fn interleaved(frames: u64, channels: ChannelCount) -> u64 {
    frames.saturating_mul(u64::from(channels.get()))
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
