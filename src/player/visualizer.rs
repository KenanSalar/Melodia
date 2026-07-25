//! The audio visualizer's sample tap.
//!
//! The visualizer needs to see what you actually *hear*, and it must never be
//! able to change it. Two pieces get that:
//!
//! - [`VisualizerShared`]: a lock-free ring the audio thread writes mono samples
//!   into and the UI thread snapshots. Pure transport — it knows nothing about
//!   FFTs, frequency bands or Slint.
//! - [`VisualizerTap`]: a Rodio [`Source`] that wraps [`EqSource`] and copies a
//!   downmixed sample out of every frame it passes through. It returns each
//!   sample *untouched*, so the audio is bit-identical whether the visualizer is
//!   on or off.
//!
//! The tap sits directly on [`EqSource`]'s output, which puts it after the EQ
//! bands, `ReplayGain`, the limiter's clamp and the crossfade ramp, but before
//! rodio's speed / pause / volume wrappers. That is the point in the chain where
//! the signal is finished but not yet scaled by the volume slider — turning the
//! volume down shouldn't flatten the bars.
//!
//! Wrapping [`EqSource`] rather than living inside it is deliberate: that module
//! carries invariants (the `frame_phase == 0` generation-poll gate, the
//! bit-identical bypass path) where a mistake is a permanent channel-parity flip
//! on a deck, and none of them have anything to do with visualisation.
//!
//! # Cadence
//!
//! One value per interleaved *frame*, not per sample — the channels of a frame
//! are one time step, and averaging them is the cheapest honest downmix. So the
//! ring fills at the source's per-channel sample rate, which is what
//! [`VisualizerShared::sample_rate`] reports.
//!
//! # Crossfade
//!
//! During an overlap both decks carry a tap and both push into the one shared
//! ring, so their samples interleave for the length of the fade and the spectrum
//! reads a little noisy. It is a second of cosmetic fuzz on a decoration; a
//! per-deck ring summed at analysis time is the exact fix if it ever matters.
//!
//! [`EqSource`]: super::equalizer::EqSource

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// Ring capacity, in mono samples. A power of two so the wrap is a mask rather
/// than a division, and comfortably wider than one analysis window so a snapshot
/// always has a full recent one to work from.
pub const RING_CAP: usize = 4096;

/// The sample ring shared between the audio thread (single writer) and the UI
/// thread (single reader).
///
/// Ownership follows [`EqShared`] / [`FadeShared`]: an `Arc`, mutated through
/// `&self`, with `f32`s held as bit patterns in atomics. It deliberately does
/// **not** carry a [`Generation`] counter — that pattern exists so an audio
/// source can poll for control-side changes, and this cell runs the other way
/// round: the audio thread writes, continuously, and the UI reads whenever it
/// feels like drawing.
///
/// A snapshot taken while the writer laps the reader can mix samples from two
/// passes of the ring. For a spectrum display that is invisible, so nothing is
/// spent defending against it.
///
/// [`EqShared`]: super::equalizer::EqShared
/// [`FadeShared`]: super::crossfade::FadeShared
/// [`Generation`]: super::dsp::Generation
pub struct VisualizerShared {
    enabled: AtomicBool,
    /// Per-channel rate (Hz) of the source currently feeding the ring, or `0`
    /// before anything has played. The analyzer needs it to place band edges,
    /// and it varies from track to track.
    sample_rate: AtomicU32,
    /// Total samples ever pushed. Monotonic — at 192 kHz a `usize` takes some
    /// millions of years to wrap, so the modulo below is the only wrapping.
    write_cursor: AtomicUsize,
    /// `RING_CAP` `f32` bit patterns.
    ring: Box<[AtomicU32]>,
}

impl VisualizerShared {
    #[must_use]
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            sample_rate: AtomicU32::new(0),
            write_cursor: AtomicUsize::new(0),
            // `AtomicU32` isn't `Clone`, so this can't be `vec![…; N]`; collecting
            // also keeps the 16 KiB off the stack on its way to the heap.
            ring: (0..RING_CAP).map(|_| AtomicU32::new(0)).collect(),
        })
    }

    // --- producer side (audio thread) --------------------------------------

    /// Append one mono sample.
    ///
    /// Wait-free and allocation-free: an atomic load, a `fetch_add` and a store.
    /// When the visualizer is off it is a single predictable branch — the ring
    /// isn't touched at all, so a disabled visualizer costs the audio thread
    /// nothing worth measuring.
    pub fn push(&self, sample: f32) {
        if !self.is_enabled() {
            return;
        }
        let idx = self.write_cursor.fetch_add(1, Ordering::Relaxed);
        self.ring[idx % RING_CAP].store(sample.to_bits(), Ordering::Relaxed);
    }

    /// Publish the rate of the source now feeding the ring.
    pub fn set_sample_rate(&self, hz: u32) {
        self.sample_rate.store(hz, Ordering::Relaxed);
    }

    // --- consumer side (UI thread) -----------------------------------------

    /// Copy the most recent `out.len()` samples into `out`, oldest first.
    ///
    /// Short history is padded at the **front**, so the newest sample is always
    /// the last element and a window that spans the start of playback ramps in
    /// out of silence. A request wider than [`RING_CAP`] is answered for the last
    /// `RING_CAP` samples only; the rest stays silent rather than repeating
    /// wrapped data.
    pub fn snapshot(&self, out: &mut [f32]) {
        let end = self.write_cursor.load(Ordering::Relaxed);
        let avail = out.len().min(RING_CAP).min(end);
        let (head, tail) = out.split_at_mut(out.len() - avail);
        head.fill(0.0);
        let start = end - avail;
        for (i, slot) in tail.iter_mut().enumerate() {
            *slot = f32::from_bits(self.ring[(start + i) % RING_CAP].load(Ordering::Relaxed));
        }
    }

    // --- both sides ---------------------------------------------------------

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Per-channel rate (Hz) of the source currently feeding the ring, or `0`
    /// if nothing has played yet.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<VisualizerShared>();
};

/// A transparent [`Source`] that copies a downmixed sample out of every frame it
/// forwards.
///
/// Every source the decks play is built through
/// [`RodioPlayer::build_source`](super::rodio_backend::RodioPlayer), which wraps
/// the track's [`EqSource`](super::equalizer::EqSource) in one of these — so the
/// playing track, a gapless successor and both sides of a crossfade all feed the
/// same ring.
pub struct VisualizerTap<S> {
    input: S,
    viz: Arc<VisualizerShared>,
    /// Channel count and its reciprocal, read once — both are constant for a
    /// decoded source's life, and the reciprocal turns the downmix into a
    /// multiply.
    channels: usize,
    inv_channels: f32,
    /// This source's per-channel rate, published on its first completed frame.
    rate_hz: u32,
    /// Running sum of the frame being assembled, and how much of it has arrived.
    accum: f32,
    phase: usize,
    rate_published: bool,
}

impl<S: Source> VisualizerTap<S> {
    pub fn new(input: S, viz: Arc<VisualizerShared>) -> Self {
        let channels = input.channels().get();
        Self {
            channels: usize::from(channels),
            // `channels` is a `NonZero<u16>`, so this is finite, and `u16 → f32`
            // is lossless.
            inv_channels: 1.0 / f32::from(channels),
            rate_hz: input.sample_rate().get(),
            input,
            viz,
            accum: 0.0,
            phase: 0,
            rate_published: false,
        }
    }
}

impl<S: Source> Iterator for VisualizerTap<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let s = self.input.next()?;
        self.accum += s;
        self.phase += 1;
        if self.phase >= self.channels {
            // Published here rather than in `new` because a gapless successor is
            // built when it is *staged*, seconds before it plays. Announcing its
            // rate then would leave the tail of a differently-rated current track
            // being analysed against the wrong one.
            if !self.rate_published {
                self.viz.set_sample_rate(self.rate_hz);
                self.rate_published = true;
            }
            self.viz.push(self.accum * self.inv_channels);
            self.accum = 0.0;
            self.phase = 0;
        }
        Some(s)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}

impl<S: Source> Source for VisualizerTap<S> {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        self.input.current_span_len()
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.input.channels()
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.input.sample_rate()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.input.try_seek(pos)?;
        // The decoder lands on a frame boundary and `EqSource::try_seek` restarts
        // its interleave phase at 0, so realign with them. Left alone, a seek
        // taken mid-frame would leave every later downmix straddling two frames.
        self.accum = 0.0;
        self.phase = 0;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/visualizer_tests.rs"]
mod tests;
