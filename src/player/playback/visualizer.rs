//! The audio visualizer's sample tap: it must see what you actually *hear* and must never be able
//! to change it.
//!
//! [`VisualizerShared`] is the transport — a lock-free ring per deck, written by the audio thread
//! and snapshotted by the UI as a mix — and [`VisualizerTap`] is an [`AudioSource`] wrapping
//! [`EqSource`] that copies one downmixed value out of each frame it forwards, leaving the audio
//! bit-identical either way.
//!
//! **`src/player/CLAUDE.md` argues the design** — where the tap sits in the chain, why there is
//! one ring per deck rather than one shared, and why the analyzer reads
//! [`VisualizerShared::analysis_rate`] rather than the media rate. What follows here is what each
//! piece has to uphold.
//!
//! One value per interleaved *frame*, so a ring fills at its source's per-channel rate.
//!
//! [`EqSource`]: super::equalizer::EqSource

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use super::decks::DECK_COUNT;
use crate::player::source::audio::{AudioSource, ChannelCount, Sample, SampleRate, SeekError};

/// Ring capacity, in mono samples. A power of two so the wrap is a mask, and comfortably wider
/// than one analysis window so a snapshot always has a full recent one to work from.
///
/// Sized for the *widest* window any style asks for, at the highest rate a music file plausibly
/// carries — the waveform's span plus its trigger slack is a fixed number of **milliseconds**, so
/// it outgrows [`FFT_SIZE`](super::spectrum::FFT_SIZE) well before the rate ceiling. Resident for
/// the life of the player and never reallocated.
pub const RING_CAP: usize = 16_384;

/// One deck's ring, plus the bookkeeping that says whether anything is filling it and how far back
/// its current run reaches.
struct DeckRing {
    /// Bracketed by [`DeckRun`], so it is non-zero exactly while a tapped source is alive on this
    /// deck — the signal [`VisualizerShared::snapshot`] mixes on.
    sources: AtomicUsize,
    /// Cursor the current run started at. Everything before it belongs to whatever this deck
    /// played *last* and must never be mixed in.
    valid_from: AtomicUsize,
    /// Total samples ever pushed, monotonic — a `usize` takes millions of years to wrap, so the
    /// modulo below is the only wrapping.
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
            // `AtomicU32` isn't `Clone`, so this can't be `vec![…; N]`; collecting also keeps the
            // buffer off the stack on its way out.
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
    /// `fetch_max` rather than a store because there are two stampers — the UI thread arming the
    /// tap, the control thread opening a run — whose read-then-write pairs can interleave, and an
    /// older cursor landing last would re-admit exactly the history the stamp excludes.
    fn drop_history(&self) {
        self.valid_from.fetch_max(self.write_cursor.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// Open a run. The first source on an idle deck drops the previous run's tail; a second one
    /// joining a live deck (a gapless successor staged behind the playing track) keeps it, because
    /// gapless audio is continuous.
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

    /// Write (or add) the most recent `out.len()` samples of this run into `out`, oldest first.
    /// Short history pads at the **front**, so the newest sample is always last and a deck that
    /// just started contributes its handful over silence — exactly what it gave the mixer.
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

/// The sample rings shared between the audio thread (one writer per deck) and the UI thread
/// (single reader).
///
/// Ownership follows [`EqShared`] / [`FadeShared`], but it deliberately carries no [`Generation`]
/// counter: that pattern exists so an audio source can poll for control-side changes, and this
/// cell runs the other way round — the audio thread writes continuously and the UI reads whenever
/// it draws.
///
/// A snapshot taken while a writer laps the reader can mix two passes of the ring. Invisible on a
/// spectrum display, so nothing is spent defending it.
///
/// [`EqShared`]: super::equalizer::EqShared
/// [`FadeShared`]: super::crossfade::FadeShared
/// [`Generation`]: super::dsp::Generation
pub struct VisualizerShared {
    enabled: AtomicBool,
    /// Per-channel rate of the source most recently started, `0` before anything has played. The
    /// analyzer places band edges against it, and it varies from track to track.
    sample_rate: AtomicU32,
    /// Live playback speed — see [`analysis_rate`](Self::analysis_rate) for why the analyzer can't
    /// use `sample_rate` alone.
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

    /// Open a run on `deck` and hand back the handle its source pushes through. An index no deck
    /// owns yields an inert handle rather than a panic on the audio thread.
    #[must_use]
    pub fn begin_run(self: &Arc<Self>, deck: usize) -> DeckRun {
        if let Some(ring) = self.decks.get(deck) {
            ring.open();
        }
        DeckRun {
            viz: self.clone(),
            deck,
            released: false,
        }
    }

    /// Publish the rate of the source now feeding a ring.
    pub fn set_sample_rate(&self, hz: u32) {
        self.sample_rate.store(hz, Ordering::Relaxed);
    }

    // --- consumer side (UI thread) -----------------------------------------

    /// Copy the most recent `out.len()` samples into `out`, oldest first, summing every deck
    /// currently being written.
    ///
    /// A request wider than [`RING_CAP`] is answered for the last `RING_CAP` samples only; the
    /// rest stays silent rather than repeating wrapped data.
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

    /// Arm or disarm the tap. Arming drops whatever history the rings hold — it runs when the
    /// Now-Playing view comes back on screen, and the newest samples down there may predate its
    /// closing. The stamps land *before* the flag, so no armed sample is thrown away.
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

    /// Publish the live playback speed. `PlaybackEngine::set_speed` is the single writer — boot
    /// hydration goes through it and `play_media` / `begin_crossfade` only re-apply what it
    /// published, so don't add calls there.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "speed is bounded to 0.25..=2.0, where f32 has far more precision than the UI's 8 presets can express"
    )]
    pub fn set_speed(&self, speed: f64) {
        self.speed.store((speed as f32).to_bits(), Ordering::Relaxed);
    }

    /// The rate the samples were *decoded* at — see [`analysis_rate`](Self::analysis_rate) for the
    /// one the analyzer wants.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// The rate the analyzer places its band edges against: the source's own rate scaled by the
    /// live playback speed.
    ///
    /// The tap sits *above* the deck's converter, which is where speed is applied. So the ring
    /// holds media samples at the media rate, and analysing against that plots the *file's* pitch
    /// rather than the one the ear hears.
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
        // Unity is the common case and must round-trip exactly; a corrupt speed falls back to the
        // unscaled rate rather than to zero, which would blank the bars.
        if base == 0 || !speed.is_finite() || speed <= 0.0 || (speed - 1.0).abs() < f32::EPSILON {
            return base;
        }
        (base as f32 * speed).round() as u32
    }
}

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<VisualizerShared>();
};

/// One source's claim on a deck's ring, held for exactly as long as that source is alive — and
/// what tells the reader which rings to mix. A deck whose last source was dropped still holds a
/// full window, and mixing that frozen tail into every later frame leaves a ghost of the track
/// that ended. A deck drops a source as soon as it is exhausted or cleared, so the claim is
/// released within a frame of the audio stopping.
///
/// It is not *strictly* ordered against a control op, and `src/player/CLAUDE.md` documents that
/// race and why neither deterministic fix was bought.
pub struct DeckRun {
    viz: Arc<VisualizerShared>,
    deck: usize,
    /// Whether [`Self::release`] has already closed the ring, so the drop behind it is a no-op.
    released: bool,
}

impl DeckRun {
    /// `begin_run` already validated the index, so the miss is unreachable — but a total lookup
    /// costs one branch and no panic path on the audio thread.
    #[inline]
    fn ring(&self) -> Option<&DeckRing> {
        self.viz.decks.get(self.deck)
    }

    /// Append one mono sample: an atomic load, a `fetch_add` and a store, wait-free. Disarmed it
    /// is a single predictable branch touching no ring at all, which is what most of a listening
    /// session runs — the tap is armed only while the Now-Playing view is on screen.
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

    /// End the run now rather than at the drop.
    ///
    /// A spent source is freed off the audio callback and so outlives the audio it was making,
    /// where this claim may not: the reader mixes every ring still claimed. Idempotent, because the
    /// drop behind it runs either way.
    pub fn release(&mut self) {
        if self.released {
            return;
        }
        if let Some(ring) = self.ring() {
            ring.close();
        }
        self.released = true;
    }
}

impl Drop for DeckRun {
    fn drop(&mut self) {
        self.release();
    }
}

/// A transparent [`AudioSource`] copying a downmixed sample out of every frame it forwards. Every
/// source the decks play is wrapped in one by
/// `engine::backend::PlaybackEngine::build_source`, so the playing track, a
/// gapless successor and both sides of a crossfade each feed the ring of the deck they were built
/// for.
pub struct VisualizerTap<S> {
    input: S,
    run: DeckRun,
    /// Read once — constant for a decoded source's life, and the reciprocal turns the downmix into
    /// a multiply.
    channels: usize,
    inv_channels: f32,
    /// This source's per-channel rate, published on its first completed frame.
    rate_hz: u32,
    /// Running sum of the frame being assembled, and how much has arrived.
    accum: f32,
    phase: usize,
    rate_published: bool,
}

impl<S: AudioSource> VisualizerTap<S> {
    pub fn new(input: S, viz: &Arc<VisualizerShared>, deck: usize) -> Self {
        let channels = input.channels().get();
        Self {
            channels: usize::from(channels),
            // `channels` is a `NonZero<u16>`, so this is finite.
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

impl<S: AudioSource> Iterator for VisualizerTap<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let s = self.input.next()?;
        self.accum += s;
        self.phase += 1;
        if self.phase >= self.channels {
            // Not in `new`: a gapless successor is built when it is *staged*, seconds before it
            // plays, so announcing its rate then would analyse the tail of a differently-rated
            // current track against it.
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

impl<S: AudioSource> AudioSource for VisualizerTap<S> {
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
        // Start the frame being averaged over. The input does resume in phase, so this re-anchors
        // a grid rather than repairing one: at worst it downmixes one straddled frame, on a
        // decoration, which is cheaper than threading the phase up through the tap.
        self.accum = 0.0;
        self.phase = 0;
        Ok(())
    }

    /// The claim this tap holds is exactly what the trait's default cannot know about: released
    /// here, so a source freed off the callback stops being mixed the moment it stops playing.
    fn release_claims(&mut self) {
        self.run.release();
        self.input.release_claims();
    }
}

#[cfg(test)]
#[path = "tests/visualizer_tests.rs"]
mod tests;
