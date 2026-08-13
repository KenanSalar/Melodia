//! The audio visualizer's sample tap.
//!
//! The visualizer must see what you actually *hear* and must never be able to
//! change it. [`VisualizerShared`] is the transport — a lock-free ring per deck,
//! written by the audio thread and snapshotted by the UI as a mix — and
//! [`VisualizerTap`] is a Rodio [`Source`] wrapping [`EqSource`] that copies one
//! downmixed value out of each frame it forwards. Samples pass through
//! untouched: the audio is bit-identical with the tap on or off.
//!
//! Tapping [`EqSource`]'s output puts it after the EQ bands, `ReplayGain`, the
//! limiter's clamp and the crossfade ramp but before rodio's speed / pause /
//! volume wrappers — the signal is finished, and turning the volume down
//! shouldn't flatten the bars. Wrapping rather than living inside that module
//! keeps its invariants (the `frame_phase == 0` poll gate, the bit-identical
//! bypass path) clear of a change with nothing to do with them.
//!
//! One value per interleaved *frame*, since a frame's channels are one time
//! step, so a ring fills at its source's per-channel rate
//! ([`VisualizerShared::sample_rate`]). The analyzer wants
//! [`VisualizerShared::analysis_rate`], which folds in the playback speed the
//! tap sits underneath.
//!
//! # Crossfade
//!
//! [`VisualizerShared::snapshot`] sums the rings being written, which is why
//! there is one per deck: rodio's mixer pulls a sample from *every* live voice
//! per output sample, so two taps sharing a ring would interleave rather than
//! sum — both tracks at half rate plus a square wave at Nyquist, for the whole
//! overlap. Each ring holds its deck's already-ramped signal, so the sum is the
//! mixer's own output. Decks at different *rates* fill at different rates
//! though, and one `sample_rate` is analysed against (the incoming track's), so
//! the outgoing one reads time-scaled by the ratio until the fade lands — under
//! a semitone, on a decoration.
//!
//! [`EqSource`]: super::equalizer::EqSource

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

use super::decks::DECK_COUNT;

/// Ring capacity, in mono samples. A power of two so the wrap is a mask rather
/// than a division, and comfortably wider than one analysis window so a snapshot
/// always has a full recent one to work from.
///
/// Sized for the *widest* window any style asks for, at the highest rate a music
/// file plausibly carries: the waveform's span plus its trigger slack is a fixed
/// number of **milliseconds**, so at 192 kHz it wants ~11.5k samples where the
/// spectrum only ever wants [`FFT_SIZE`](super::spectrum::FFT_SIZE). At 4 bytes
/// a slot this is 64 KiB per deck, resident for the life of the player and never
/// reallocated.
pub const RING_CAP: usize = 16_384;

/// One deck's ring, plus the bookkeeping that says whether anything is currently
/// filling it and how far back its current run reaches.
struct DeckRing {
    /// Sources feeding this ring right now. Bracketed by [`DeckRun`], so it is
    /// non-zero exactly while a tapped source is alive on this deck — the signal
    /// [`VisualizerShared::snapshot`] mixes on.
    sources: AtomicUsize,
    /// Cursor value the current run started at. Everything before it belongs to
    /// whatever this deck played *last*, which is a different track from a
    /// different time and must never be mixed in.
    valid_from: AtomicUsize,
    /// Total samples ever pushed. Monotonic — at 192 kHz a `usize` takes some
    /// millions of years to wrap, so the modulo below is the only wrapping.
    write_cursor: AtomicUsize,
    /// `RING_CAP` `f32` bit patterns.
    ring: Box<[AtomicU32]>,
}

impl DeckRing {
    fn new() -> Self {
        Self {
            sources: AtomicUsize::new(0),
            valid_from: AtomicUsize::new(0),
            write_cursor: AtomicUsize::new(0),
            // `AtomicU32` isn't `Clone`, so this can't be `vec![…; N]`; collecting
            // also keeps the 64 KiB off the stack on its way to the heap.
            ring: (0..RING_CAP).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    #[inline]
    fn push(&self, sample: f32) {
        let idx = self.write_cursor.fetch_add(1, Ordering::Relaxed);
        self.ring[idx % RING_CAP].store(sample.to_bits(), Ordering::Relaxed);
    }

    /// Forget everything written so far, so the next window starts from silence.
    ///
    /// `fetch_max` rather than a store, because there are two stampers: the UI
    /// thread marks every ring when it arms the tap, and the control thread marks
    /// one when it opens a run. Their read-then-write pairs can interleave, and an
    /// older cursor landing last would re-admit exactly the history the stamp
    /// exists to exclude. A stamp only ever needs to move forward, so keeping the
    /// newer one costs nothing and neither can undo the other.
    fn drop_history(&self) {
        self.valid_from.fetch_max(self.write_cursor.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// Open a run. The first source on an idle deck drops the previous run's
    /// tail; a second one joining a live deck (a gapless successor staged behind
    /// the playing track) keeps it, because gapless audio is continuous.
    ///
    /// The `Release` publishes the stamp above to whoever loads `sources` next.
    fn open(&self) {
        if self.sources.load(Ordering::Relaxed) == 0 {
            self.drop_history();
        }
        self.sources.fetch_add(1, Ordering::Release);
    }

    fn close(&self) {
        self.sources.fetch_sub(1, Ordering::Release);
    }

    fn is_live(&self) -> bool {
        self.sources.load(Ordering::Acquire) > 0
    }

    #[inline]
    fn sample_at(&self, cursor: usize) -> f32 {
        f32::from_bits(self.ring[cursor % RING_CAP].load(Ordering::Relaxed))
    }

    /// Write (or add) the most recent `out.len()` samples of this run into `out`,
    /// oldest first. Short history is padded at the **front**, so the newest
    /// sample is always the last element — a deck that started a moment ago
    /// contributes its handful of samples over silence, which is exactly what it
    /// contributed to the mixer.
    fn read_into(&self, out: &mut [f32], add: bool) {
        let end = self.write_cursor.load(Ordering::Relaxed);
        let run = end.saturating_sub(self.valid_from.load(Ordering::Relaxed));
        let avail = out.len().min(RING_CAP).min(run);
        let (head, tail) = out.split_at_mut(out.len() - avail);
        let start = end - avail;
        if add {
            for (i, slot) in tail.iter_mut().enumerate() {
                *slot += self.sample_at(start + i);
            }
        } else {
            head.fill(0.0);
            for (i, slot) in tail.iter_mut().enumerate() {
                *slot = self.sample_at(start + i);
            }
        }
    }
}

/// The sample rings shared between the audio thread (one writer per deck) and
/// the UI thread (single reader).
///
/// Ownership follows [`EqShared`] / [`FadeShared`]: an `Arc`, mutated through
/// `&self`, with `f32`s held as bit patterns in atomics. It deliberately does
/// **not** carry a [`Generation`] counter — that pattern exists so an audio
/// source can poll for control-side changes, and this cell runs the other way
/// round: the audio thread writes, continuously, and the UI reads whenever it
/// feels like drawing.
///
/// A snapshot taken while a writer laps the reader can mix samples from two
/// passes of the ring. For a spectrum display that is invisible, so nothing is
/// spent defending against it.
///
/// [`EqShared`]: super::equalizer::EqShared
/// [`FadeShared`]: super::crossfade::FadeShared
/// [`Generation`]: super::dsp::Generation
pub struct VisualizerShared {
    enabled: AtomicBool,
    /// Per-channel rate (Hz) of the source most recently started, or `0` before
    /// anything has played. The analyzer needs it to place band edges, and it
    /// varies from track to track.
    sample_rate: AtomicU32,
    /// Live playback speed as `f32` bits. Written by the control side, read by
    /// the UI — see [`analysis_rate`](Self::analysis_rate) for why the analyzer
    /// can't use `sample_rate` alone.
    speed: AtomicU32,
    /// One ring per deck, indexed by [`Deck::viz_slot`](super::decks::Deck).
    decks: [DeckRing; DECK_COUNT],
}

impl VisualizerShared {
    #[must_use]
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            sample_rate: AtomicU32::new(0),
            speed: AtomicU32::new(1.0_f32.to_bits()),
            decks: std::array::from_fn(|_| DeckRing::new()),
        })
    }

    // --- producer side (audio thread) --------------------------------------

    /// Open a run on `deck` and hand back the handle its source pushes through.
    ///
    /// An index no deck owns yields a handle that does nothing, rather than a
    /// panic on the audio thread.
    #[must_use]
    pub fn begin_run(self: &Arc<Self>, deck: usize) -> DeckRun {
        if let Some(ring) = self.decks.get(deck) {
            ring.open();
        }
        DeckRun {
            viz: self.clone(),
            deck,
        }
    }

    /// Publish the rate of the source now feeding a ring.
    pub fn set_sample_rate(&self, hz: u32) {
        self.sample_rate.store(hz, Ordering::Relaxed);
    }

    // --- consumer side (UI thread) -----------------------------------------

    /// Copy the most recent `out.len()` samples into `out`, oldest first,
    /// summing every deck currently being written.
    ///
    /// Outside a crossfade exactly one deck is live and this is a plain copy of
    /// its window. During one it is the mixer's own sum — see the module docs.
    /// A request wider than [`RING_CAP`] is answered for the last `RING_CAP`
    /// samples only; the rest stays silent rather than repeating wrapped data.
    pub fn snapshot(&self, out: &mut [f32]) {
        let mut written = false;
        for deck in &self.decks {
            if !deck.is_live() {
                continue;
            }
            deck.read_into(out, written);
            written = true;
        }
        if !written {
            out.fill(0.0);
        }
    }

    // --- both sides ---------------------------------------------------------

    /// Arm or disarm the tap.
    ///
    /// Arming drops whatever history the rings still hold: it is called when the
    /// Now-Playing view comes back on screen, and the newest samples down there
    /// may be from before it was closed. The stamps land *before* the flag, so no
    /// armed sample is thrown away.
    pub fn set_enabled(&self, on: bool) {
        if on && !self.is_enabled() {
            for deck in &self.decks {
                deck.drop_history();
            }
        }
        self.enabled.store(on, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Publish the live playback speed. Takes `f64` to match the rest of the
    /// player API; the cell is `f32` because that's what the analysis is in.
    ///
    /// `RodioPlayer::set_speed` is the single writer: boot hydration goes
    /// through it, and `play_media` / `begin_crossfade` only ever re-apply the
    /// value it already published. Don't add redundant calls there.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "speed is bounded to 0.25..=2.0, where f32 has far more precision than the UI's 8 presets can express"
    )]
    pub fn set_speed(&self, speed: f64) {
        self.speed.store((speed as f32).to_bits(), Ordering::Relaxed);
    }

    /// Per-channel rate (Hz) of the source most recently started, or `0` if
    /// nothing has played yet. The rate the samples were *decoded* at — see
    /// [`analysis_rate`](Self::analysis_rate) for the one the analyzer wants.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// The rate the analyzer should place its band edges against: the source's
    /// own rate scaled by the live playback speed.
    ///
    /// The tap sits *inside* rodio's `Speed` wrapper, which forwards samples
    /// verbatim and implements speed purely by reporting a multiplied
    /// `sample_rate()` upward — the mixer then resamples. So the ring holds the
    /// media samples at the media rate, and analysing them against that rate
    /// would plot the *file's* pitch: at 2× a 1 kHz tone is heard an octave up
    /// while the 1 kHz bar lights. Scaling here puts the bars back on what you
    /// actually hear.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "audio rates are well below 2^24 Hz and speed is bounded to 0.25..=2.0, so the product is exact in f32 and far inside u32"
    )]
    #[must_use]
    pub fn analysis_rate(&self) -> u32 {
        let base = self.sample_rate();
        let speed = f32::from_bits(self.speed.load(Ordering::Relaxed));
        // Unity is the overwhelmingly common case and must round-trip exactly;
        // a corrupt speed falls back to the unscaled rate rather than a rate of
        // zero, which would blank the bars.
        if base == 0 || !speed.is_finite() || speed <= 0.0 || (speed - 1.0).abs() < f32::EPSILON {
            return base;
        }
        (base as f32 * speed).round() as u32
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<VisualizerShared>();
};

/// One source's claim on a deck's ring, held for exactly as long as that source
/// is alive.
///
/// The claim is what tells the reader which rings to mix: a deck whose last
/// source has been dropped still holds a full window of audio, and mixing that
/// frozen tail into every later frame would leave a ghost of the track that
/// ended. rodio drops a source as soon as it is exhausted or cleared, so the
/// claim is released within a frame of the audio stopping — close enough to key
/// the mix on.
///
/// It is not *strictly* ordered against a control op, though, and one case is
/// worth knowing: `Player::clear()` returns on the queue's end-of-source signal,
/// which rodio sends a couple of instructions before it replaces (and so drops)
/// the source. So a hard cut, which clears and then opens a run on the same deck,
/// can find the outgoing claim still standing and skip its history drop — leaving
/// the incoming track's first window carrying the tail of the outgoing one, for
/// as long as the widest window takes to refill. Cosmetic, self-healing, and
/// exactly what a single shared ring did all the time; both deterministic fixes
/// (threading a fresh-vs-continuing flag through every `build_source` call site,
/// or handing `Deck` the shared cell so `reset` can stamp) cost more structure
/// than the fault is worth.
pub struct DeckRun {
    viz: Arc<VisualizerShared>,
    deck: usize,
}

impl DeckRun {
    /// The ring this run claimed, or `None` for a deck index that doesn't
    /// exist. `begin_run` already validated it, so the miss is unreachable —
    /// but keeping the lookup total costs one branch and no panic path on the
    /// audio thread.
    #[inline]
    fn ring(&self) -> Option<&DeckRing> {
        self.viz.decks.get(self.deck)
    }

    /// Append one mono sample.
    ///
    /// Wait-free and allocation-free: an atomic load, a `fetch_add` and a store.
    /// When the tap is disarmed it is a single predictable branch — no ring is
    /// touched at all. That matters more than it looks: the tap is armed only
    /// while the Now-Playing view is on screen (see `crate::ui::visualizer`), so
    /// for most of a listening session this is the branch and nothing else.
    #[inline]
    pub fn push(&self, sample: f32) {
        if !self.viz.is_enabled() {
            return;
        }
        if let Some(ring) = self.ring() {
            ring.push(sample);
        }
    }

    /// Publish the rate of the source holding this run.
    pub fn set_sample_rate(&self, hz: u32) {
        self.viz.set_sample_rate(hz);
    }
}

impl Drop for DeckRun {
    fn drop(&mut self) {
        if let Some(ring) = self.ring() {
            ring.close();
        }
    }
}

/// A transparent [`Source`] that copies a downmixed sample out of every frame it
/// forwards.
///
/// Every source the decks play is built through
/// [`RodioPlayer::build_source`](super::rodio_backend::RodioPlayer), which wraps
/// the track's [`EqSource`](super::equalizer::EqSource) in one of these — so the
/// playing track, a gapless successor and both sides of a crossfade all feed a
/// ring, each the one belonging to the deck it was built for.
pub struct VisualizerTap<S> {
    input: S,
    run: DeckRun,
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
    pub fn new(input: S, viz: &Arc<VisualizerShared>, deck: usize) -> Self {
        let channels = input.channels().get();
        Self {
            channels: usize::from(channels),
            // `channels` is a `NonZero<u16>`, so this is finite, and `u16 → f32`
            // is lossless.
            inv_channels: 1.0 / f32::from(channels),
            rate_hz: input.sample_rate().get(),
            run: viz.begin_run(deck),
            input,
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
                self.run.set_sample_rate(self.rate_hz);
                self.rate_published = true;
            }
            self.run.push(self.accum * self.inv_channels);
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
