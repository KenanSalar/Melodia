//! Graphic-equalizer DSP.
//!
//! Rodio 0.22 ships only `low_pass` / `high_pass` BLT filters — no
//! peaking/parametric filters — so a real graphic EQ can't be built from its
//! primitives. This module provides:
//!
//! - [`EqShared`]: lock-free state (per-band gains + an enabled flag) read on
//!   the audio thread. The library/UI layer mutates it; the audio thread polls
//!   a generation counter and only recomputes coefficients when it changes.
//! - [`EqSource`]: a custom Rodio [`Source`] wrapping a decoder. Each sample
//!   runs through a cascade of ten `Type::PeakingEQ` biquads — one
//!   [`DirectForm1`] per band, **per channel** (rodio's own `BltFilter` keeps a
//!   single filter state across interleaved channels, which is part of why it's
//!   "probably buggy"; we keep independent per-channel state).
//!
//! `DirectForm1` is used (not `DirectForm2Transposed`) because its delay line
//! holds past inputs/outputs that stay valid when coefficients change at
//! runtime, so live slider drags swap coefficients without injecting transients.
//!
//! When the EQ is disabled or every band sits at unity gain, [`EqSource`] is a
//! transparent passthrough — **zero added DSP cost in the default-off state**.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use biquad::{Biquad, Coefficients, DirectForm1, Hertz, Type};
use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// Number of equalizer bands.
pub const NUM_BANDS: usize = 10;

/// ISO octave centre frequencies (Hz) for the ten bands, low → high.
pub const BAND_FREQS: [f32; NUM_BANDS] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// Per-band gain range, in decibels.
pub const MIN_GAIN_DB: f32 = -12.0;
pub const MAX_GAIN_DB: f32 = 12.0;

/// Quality factor for the peaking filters. ~1.4 gives musically sensible
/// overlap for octave-spaced bands without ringing.
const BAND_Q: f32 = 1.41;

/// Bands within this many dB of unity are treated as off (skipped in the
/// filter chain) so a near-flat EQ stays close to free.
const GAIN_EPSILON_DB: f32 = 0.05;

/// A named gain curve. `PRESETS` order is load-bearing: the Slint preset
/// dropdown lists the same names as inline `@tr` literals in this order, and
/// the UI maps the dropdown index straight into this slice.
pub struct EqPreset {
    pub name: &'static str,
    pub gains: [f32; NUM_BANDS],
}

/// Built-in presets. `Flat` (index 0) is the neutral default; the UI appends a
/// synthetic "Custom" entry after these for hand-tuned curves.
pub const PRESETS: [EqPreset; 9] = [
    EqPreset { name: "Flat", gains: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
    EqPreset { name: "Rock", gains: [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 1.0, 3.0, 4.0, 4.0] },
    EqPreset { name: "Pop", gains: [-1.0, 0.0, 1.0, 2.0, 3.0, 3.0, 2.0, 1.0, 0.0, -1.0] },
    EqPreset { name: "Jazz", gains: [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0] },
    EqPreset { name: "Classical", gains: [4.0, 3.0, 2.0, 1.0, -1.0, -1.0, 0.0, 2.0, 3.0, 4.0] },
    EqPreset { name: "Bass Boost", gains: [6.0, 5.0, 4.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0] },
    EqPreset { name: "Treble Boost", gains: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0] },
    EqPreset { name: "Vocal", gains: [-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0, -1.0] },
    EqPreset { name: "Electronic", gains: [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 1.0, 3.0, 5.0] },
];

/// Number of built-in presets. The UI's synthetic "Custom" dropdown entry
/// sits at this index (one past the last built-in).
pub const PRESET_COUNT: usize = PRESETS.len();

/// Default preset name persisted on first launch.
pub const DEFAULT_PRESET: &str = "Flat";

/// Clamp a single band gain into the supported range.
#[must_use]
pub fn clamp_gain(db: f32) -> f32 {
    if db.is_nan() {
        0.0
    } else {
        db.clamp(MIN_GAIN_DB, MAX_GAIN_DB)
    }
}

/// Coerce an arbitrary (possibly hand-edited / wrong-length) gain list into a
/// validated `[f32; NUM_BANDS]`: pad missing bands with 0, drop extras, clamp.
#[must_use]
pub fn normalize_gains(gains: &[f32]) -> [f32; NUM_BANDS] {
    std::array::from_fn(|i| clamp_gain(gains.get(i).copied().unwrap_or(0.0)))
}

/// Index of a named preset, if it matches one of [`PRESETS`].
#[must_use]
pub fn preset_index(name: &str) -> Option<usize> {
    PRESETS.iter().position(|p| p.name == name)
}

/// Lock-free equalizer state shared between the control layer (writer) and the
/// audio thread (reader). Gains are stored as `f32` bit patterns in atomics;
/// every mutation bumps `generation` so [`EqSource`] knows to recompute.
pub struct EqShared {
    enabled: AtomicBool,
    gains_bits: [AtomicU32; NUM_BANDS],
    generation: AtomicU64,
}

impl EqShared {
    /// Build shared state seeded from persisted settings. `generation` starts
    /// at 1 so a freshly constructed [`EqSource`] (which seeds its cached
    /// generation to a different value) always rebuilds before its first sample.
    #[must_use]
    pub fn new(enabled: bool, gains: &[f32]) -> Arc<Self> {
        let norm = normalize_gains(gains);
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            gains_bits: std::array::from_fn(|i| AtomicU32::new(norm[i].to_bits())),
            generation: AtomicU64::new(1),
        })
    }

    /// Publish a state change. `Release` here pairs with the reader's `Acquire`
    /// load of `generation` so the gain/enabled writes that precede it are
    /// visible once the reader observes the new generation.
    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        self.bump();
    }

    pub fn set_gain(&self, index: usize, db: f32) {
        if let Some(cell) = self.gains_bits.get(index) {
            cell.store(clamp_gain(db).to_bits(), Ordering::Relaxed);
            self.bump();
        }
    }

    pub fn set_all_gains(&self, gains: &[f32]) {
        let norm = normalize_gains(gains);
        for (cell, g) in self.gains_bits.iter().zip(norm) {
            cell.store(g.to_bits(), Ordering::Relaxed);
        }
        self.bump();
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn gain(&self, index: usize) -> f32 {
        self.gains_bits
            .get(index)
            .map_or(0.0, |c| f32::from_bits(c.load(Ordering::Relaxed)))
    }

    #[must_use]
    pub fn gains(&self) -> [f32; NUM_BANDS] {
        std::array::from_fn(|i| f32::from_bits(self.gains_bits[i].load(Ordering::Relaxed)))
    }

    #[must_use]
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

/// Transparent passthrough coefficients (`y[n] = x[n]`). Only used to
/// construct the filter banks; [`EqSource::rebuild`] overwrites a band's
/// coefficients before that band is ever run.
fn identity_coeffs() -> Coefficients<f32> {
    Coefficients { a1: 0.0, a2: 0.0, b0: 1.0, b1: 0.0, b2: 0.0 }
}

/// A Rodio source that applies the shared graphic EQ to its inner decoder.
///
/// One [`EqSource`] wraps each decoded track; both the playing track and the
/// gapless-preloaded one share the same [`EqShared`], so a live change applies
/// to both. Per-source state (the filter banks) is a few hundred bytes — no
/// caches, negligible memory.
pub struct EqSource<S> {
    input: S,
    shared: Arc<EqShared>,
    /// Last generation this source applied; mismatch triggers `rebuild`.
    last_generation: u64,
    /// This source's sample rate (Hz), captured once — constant per decoded
    /// file, so coefficients computed from it stay correct for the source's life.
    sample_rate: f32,
    channels: usize,
    /// Which interleaved channel the next sample belongs to.
    cursor: usize,
    /// `[channel][band]` filter state. Fixed size; allocated once.
    banks: Vec<[DirectForm1<f32>; NUM_BANDS]>,
    /// Per-band on/off — a band is inactive at unity gain or outside Nyquist.
    band_active: [bool; NUM_BANDS],
    /// Fast path: when true, `next` returns the inner sample untouched.
    bypass: bool,
}

impl<S: Source> EqSource<S> {
    pub fn new(input: S, shared: Arc<EqShared>) -> Self {
        let channels = usize::from(input.channels().get());
        #[allow(
            clippy::cast_precision_loss,
            reason = "audio sample rates are well below 2^24 Hz and convert to f32 exactly"
        )]
        let sample_rate = input.sample_rate().get() as f32;
        let banks = (0..channels)
            .map(|_| std::array::from_fn(|_| DirectForm1::<f32>::new(identity_coeffs())))
            .collect();
        // Seed the cached generation to something other than the live value so
        // the first `next()` rebuilds from the current shared state.
        let last_generation = shared.generation().wrapping_sub(1);
        Self {
            input,
            shared,
            last_generation,
            sample_rate,
            channels,
            cursor: 0,
            banks,
            band_active: [false; NUM_BANDS],
            bypass: true,
        }
    }

    /// Recompute per-band coefficients from the shared state. Called only when
    /// the generation counter advances (toggle, slider drag, preset, reset).
    fn rebuild(&mut self) {
        if !self.shared.enabled() {
            self.bypass = true;
            self.band_active = [false; NUM_BANDS];
            return;
        }

        let Ok(fs) = Hertz::<f32>::from_hz(self.sample_rate) else {
            // Pathological sample rate — leave audio untouched.
            self.bypass = true;
            self.band_active = [false; NUM_BANDS];
            return;
        };

        let mut any_active = false;
        for band in 0..NUM_BANDS {
            let gain = self.shared.gain(band);

            // Skip unity-gain bands and any band whose centre exceeds this
            // source's Nyquist limit (low-sample-rate files).
            let coeffs = if gain.abs() < GAIN_EPSILON_DB {
                None
            } else {
                Hertz::<f32>::from_hz(BAND_FREQS[band]).ok().and_then(|f0| {
                    Coefficients::<f32>::from_params(Type::PeakingEQ(gain), fs, f0, BAND_Q).ok()
                })
            };

            match coeffs {
                Some(c) => {
                    let was_active = self.band_active[band];
                    for bank in &mut self.banks {
                        // inactive→active: start from a clean delay line so
                        // stale state can't pop. Already-active: keep state
                        // across the coefficient swap (DirectForm1 is safe for
                        // live coefficient changes).
                        if !was_active {
                            bank[band].reset_state();
                        }
                        bank[band].update_coefficients(c);
                    }
                    self.band_active[band] = true;
                    any_active = true;
                }
                None => self.band_active[band] = false,
            }
        }

        self.bypass = !any_active;
    }
}

impl<S: Source> Iterator for EqSource<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let generation = self.shared.generation();
        if generation != self.last_generation {
            self.rebuild();
            self.last_generation = generation;
        }

        let sample = self.input.next()?;

        // Advance the channel cursor even while bypassed so toggling the EQ on
        // mid-stream never swaps L/R alignment.
        let channel = self.cursor;
        self.cursor += 1;
        if self.cursor >= self.channels {
            self.cursor = 0;
        }

        if self.bypass {
            return Some(sample);
        }

        let bank = &mut self.banks[channel];
        let mut out = sample;
        for (filter, active) in bank.iter_mut().zip(self.band_active.iter()) {
            if *active {
                out = filter.run(out);
            }
        }
        Some(out)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.input.size_hint()
    }
}

impl<S: Source> Source for EqSource<S> {
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
        // Clear every delay line so the seek destination doesn't pop from
        // pre-seek filter history, and re-align the channel cursor (rodio seeks
        // land on a frame boundary).
        for bank in &mut self.banks {
            for filter in bank.iter_mut() {
                filter.reset_state();
            }
        }
        self.cursor = 0;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/equalizer_tests.rs"]
mod tests;
