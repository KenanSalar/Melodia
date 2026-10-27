//! The visualizer's tap: it must see what you hear and never change it. [`VisualizerShared`] is a
//! lock-free ring per deck the UI snapshots as a mix, and [`VisualizerTap`] wraps [`EqSource`],
//! copying one downmixed value per frame and leaving the audio bit-identical.
//! Above the deck's speed and volume, so the slider can't flatten the bars and the analysis owes
//! [`VisualizerShared::analysis_rate`]; a ring per deck, or a crossfade's two taps interleave.
//!
//! [`EqSource`]: super::equalizer::EqSource

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::decks::DECK_COUNT;
use melodia_audio::player::source::audio::{
    AudioSource, ChannelCount, Sample, SampleRate, SeekError, SourceFormat,
};

/// Mono samples, a power of two so the wrap is a mask. Sized for the widest window any style asks
/// for at the highest plausible rate, the waveform's millisecond span outgrowing
/// [`FFT_SIZE`](super::spectrum::FFT_SIZE) first; allocated on the first arm and kept.
pub const RING_CAP: usize = 16_384;

struct DeckRing {
    /// Non-zero exactly while a tapped source is alive on this deck, bracketed by [`DeckRun`].
    sources: AtomicUsize,
    /// Where the current run started; everything before is the deck's last track.
    valid_from: AtomicUsize,
    /// Monotonic, the modulo being the only wrapping.
    write_cursor: AtomicUsize,
    /// Built by [`Self::arm`] on the UI thread; the audio thread only `get`s, never waiting on it.
    ring: OnceLock<Box<[AtomicU32]>>,
}

impl DeckRing {
    fn new() -> Self {
        Self {
            sources: AtomicUsize::new(0),
            valid_from: AtomicUsize::new(0),
            write_cursor: AtomicUsize::new(0),
            ring: OnceLock::new(),
        }
    }

    fn arm(&self) {
        // `AtomicU32` isn't `Clone`, so this can't be `vec![…; N]`; collecting also keeps the
        // buffer off the stack on its way out.
        self.ring.get_or_init(|| (0..RING_CAP).map(|_| AtomicU32::new(0)).collect());
    }

    #[inline]
    fn push(&self, sample: f32) {
        let Some(ring) = self.ring.get() else {
            return;
        };
        let idx = self.write_cursor.fetch_add(1, Ordering::Relaxed);
        ring[idx % RING_CAP].store(sample.to_bits(), Ordering::Relaxed);
    }

    /// `fetch_max` because the UI's arm and the control thread's run open can interleave, and an
    /// older cursor landing last would re-admit the history the stamp excludes.
    fn drop_history(&self) {
        self.valid_from.fetch_max(self.write_cursor.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// The first source on an idle deck drops the last run's tail; a gapless successor joining a
    /// live deck keeps it, the audio being continuous. The `Release` publishes the stamp.
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
    fn sample_at(ring: &[AtomicU32], cursor: usize) -> f32 {
        f32::from_bits(ring[cursor % RING_CAP].load(Ordering::Relaxed))
    }

    /// Oldest first, short history padding the front, so a deck just started adds its handful over
    /// silence, as it did to the mixer.
    fn read_into(&self, out: &mut [f32], add: bool) {
        let Some(ring) = self.ring.get() else {
            if !add {
                out.fill(0.0);
            }
            return;
        };
        let end = self.write_cursor.load(Ordering::Relaxed);
        let run = end.saturating_sub(self.valid_from.load(Ordering::Relaxed));
        let avail = out.len().min(RING_CAP).min(run);
        let (head, tail) = out.split_at_mut(out.len() - avail);
        let start = end - avail;
        if add {
            for (i, slot) in tail.iter_mut().enumerate() {
                *slot += Self::sample_at(ring, start + i);
            }
        } else {
            head.fill(0.0);
            for (i, slot) in tail.iter_mut().enumerate() {
                *slot = Self::sample_at(ring, start + i);
            }
        }
    }
}

/// The rings, one writer per deck and one reader. No [`Generation`], the flow running the other way
/// round: the audio thread writes continuously and the UI reads when it draws. A snapshot racing a
/// lapping writer can mix two passes, invisible on a decoration.
///
/// [`Generation`]: super::dsp::Generation
pub struct VisualizerShared {
    enabled: AtomicBool,
    /// The most recently started source's per-channel rate, `0` before anything has played.
    sample_rate: AtomicU32,
    speed: AtomicU32,
    /// One ring per deck, indexed by [`Deck::viz_slot`](super::decks::Deck).
    decks: [DeckRing; DECK_COUNT],
}

impl VisualizerShared {
    #[must_use]
    pub fn new(enabled: bool) -> Arc<Self> {
        let shared = Self {
            enabled: AtomicBool::new(enabled),
            sample_rate: AtomicU32::new(0),
            speed: AtomicU32::new(1.0_f32.to_bits()),
            decks: std::array::from_fn(|_| DeckRing::new()),
        };
        if enabled {
            for deck in &shared.decks {
                deck.arm();
            }
        }
        Arc::new(shared)
    }

    /// An index no deck owns yields an inert handle rather than a panic on the audio thread.
    #[must_use]
    pub fn begin_run(self: &Arc<Self>, deck: usize) -> DeckRun {
        if let Some(ring) = self.decks.get(deck) {
            ring.open();
        }
        DeckRun { viz: self.clone(), deck, released: false }
    }

    pub fn set_sample_rate(&self, hz: u32) {
        self.sample_rate.store(hz, Ordering::Relaxed);
    }

    /// Oldest first, summing every live deck. Past [`RING_CAP`] the rest stays silent rather than
    /// repeating wrapped data.
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

    /// Arming builds the rings the first time and drops their history, which may predate Now
    /// Playing's close. The stamps land before the flag, so no armed sample is thrown away.
    pub fn set_enabled(&self, on: bool) {
        if on && !self.is_enabled() {
            for deck in &self.decks {
                deck.arm();
                deck.drop_history();
            }
        }
        self.enabled.store(on, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// `PlaybackEngine::set_speed` is the single writer; don't add calls beside it.
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

    /// The source rate scaled by speed: the tap sits above the converter, so the media rate would
    /// plot the file's pitch rather than the one heard.
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

/// One source's claim on its deck's ring, which tells the reader what to mix: a dead deck's frozen
/// tail would ghost into every later frame. Released within a frame of the audio stopping, though
/// not strictly ordered against a control op.
pub struct DeckRun {
    viz: Arc<VisualizerShared>,
    deck: usize,
    released: bool,
}

impl DeckRun {
    /// Total, so the audio thread has no panic path.
    #[inline]
    fn ring(&self) -> Option<&DeckRing> {
        self.viz.decks.get(self.deck)
    }

    /// Wait-free; disarmed, one branch touching no ring, which is most of a session.
    #[inline]
    pub fn push(&self, sample: f32) {
        if !self.viz.is_enabled() {
            return;
        }
        if let Some(ring) = self.ring() {
            ring.push(sample);
        }
    }

    pub fn set_sample_rate(&self, hz: u32) {
        self.viz.set_sample_rate(hz);
    }

    /// Ends the run before the drop: a spent source is freed off the callback and outlives its
    /// audio, where the claim may not. Idempotent.
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

/// Wraps every source the decks play (`PlaybackEngine::build_source`), so each feeds the ring of
/// the deck it was built for.
pub struct VisualizerTap<S> {
    input: S,
    run: DeckRun,
    /// The reciprocal makes the downmix a multiply.
    channels: usize,
    inv_channels: f32,
    /// Published on the first completed frame.
    rate_hz: u32,
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
            // Not in `new`: a gapless successor is built seconds before it plays, and its rate
            // would mis-analyse the current track's tail.
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

    fn format(&self) -> SourceFormat {
        self.input.format()
    }

    fn dsp_engaged(&self) -> bool {
        self.input.dsp_engaged()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.input.try_seek(pos)?;
        // Re-anchors the averaging grid; at worst one straddled frame downmixes, on a decoration.
        self.accum = 0.0;
        self.phase = 0;
        Ok(())
    }

    /// So a source freed off the callback stops being mixed the moment it stops playing.
    fn release_claims(&mut self) {
        self.run.release();
        self.input.release_claims();
    }
}

#[cfg(test)]
#[path = "tests/visualizer_tests.rs"]
mod tests;
