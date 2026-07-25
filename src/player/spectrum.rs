//! Spectrum analysis for the audio visualizer.
//!
//! Turns a window of raw samples — whatever [`VisualizerShared::snapshot`] last
//! copied out of the ring — into the per-band bar heights the UI draws. Pure
//! DSP: nothing here knows about rodio, Slint or threads, and every step is a
//! free function taking slices, so the maths is unit-tested directly rather than
//! through a running player.
//!
//! The pipeline, once per drawn frame:
//!
//! 1. multiply the window by a precomputed **Hann** table (cuts the spectral
//!    leakage a rectangular window would smear across neighbouring bands),
//! 2. a real-to-complex **FFT** into `FFT_SIZE / 2 + 1` bins,
//! 3. fold those bins into [`NUM_BANDS`] **geometric** bands — pitch perception
//!    is geometric, so linear bins would spend most of the display on treble,
//! 4. compress each band's magnitude to a 0..1 height on a decibel scale,
//! 5. **peak-follow smoothing** — rise at once, fall gently — so the bars read as
//!    lively rather than twitchy.
//!
//! [`SpectrumAnalyzer`] is the only stateful piece, and it exists purely to hold
//! what must not be rebuilt every frame: the FFT plan, its three buffers, the
//! Hann table, the bin→band map and the smoothed levels. Nothing in the analysis
//! path allocates.
//!
//! [`VisualizerShared::snapshot`]: super::visualizer::VisualizerShared::snapshot

use std::ops::Range;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use super::dsp::linear_to_db;

/// Samples per analysis window. At 44.1 kHz this is ~46 ms — long enough to
/// resolve bass, short enough to keep the bars in step with what you hear. A
/// power of two hits the FFT planner's fastest path.
pub const FFT_SIZE: usize = 2048;

/// Bars drawn across the display.
pub const NUM_BANDS: usize = 32;

/// Bottom edge of the lowest band. Below this is inaudible rumble and, at bin 0,
/// any DC offset — neither belongs in a bass bar.
const MIN_HZ: f32 = 20.0;

/// Level mapped to a bar height of zero. Everything from here up to full scale
/// is spread across the bar, so this sets how much quiet detail is visible.
const FLOOR_DB: f32 = -70.0;

/// Fraction of the remaining distance a bar *keeps* when rising: `0.0` snaps
/// straight to a new peak, which is what makes a transient read as a hit.
const ATTACK: f32 = 0.0;

/// Fraction of its height a bar keeps per frame while falling — a bar never
/// drops below the band's current level, it just takes its time getting there.
const DECAY: f32 = 0.8;

// --- casts -------------------------------------------------------------------

#[allow(
    clippy::cast_precision_loss,
    reason = "window and bin indices are counts in the low thousands, which convert to f32 exactly"
)]
fn index_to_f32(i: usize) -> f32 {
    i as f32
}

#[allow(
    clippy::cast_precision_loss,
    reason = "audio sample rates are well below 2^24 Hz and convert to f32 exactly"
)]
fn rate_to_f32(hz: u32) -> f32 {
    hz as f32
}

/// The FFT bin a frequency falls in, saturating at `max_bin`. Non-finite or
/// negative inputs answer bin 0.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped into 0..=max_bin before the cast, so it is non-negative and small"
)]
fn hz_to_bin(hz: f32, fft_size: usize, sample_rate: f32, max_bin: usize) -> usize {
    let bin = hz * index_to_f32(fft_size) / sample_rate;
    if !bin.is_finite() || bin <= 0.0 {
        return 0;
    }
    bin.min(index_to_f32(max_bin)) as usize
}

// --- pure DSP ----------------------------------------------------------------

/// A Hann window of `size` points: `0.5 · (1 − cos(2πi / (size − 1)))`.
///
/// Zero at both ends, one in the middle, symmetric. Built once and reused —
/// it depends on nothing but its length.
#[must_use]
pub fn hann_window(size: usize) -> Box<[f32]> {
    // A one-point window has no span to taper across, and the formula would
    // divide by zero, so it degenerates to a passthrough.
    if size < 2 {
        return vec![1.0; size].into_boxed_slice();
    }
    let denom = index_to_f32(size - 1);
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * index_to_f32(i) / denom).cos()))
        .collect()
}

/// The magnitude scale that puts a full-scale sine at 1.0.
///
/// A windowed sine of amplitude `A` peaks at `A/2 · Σw` in the bin it lands in,
/// so dividing by half the window's sum normalizes full scale to unity — the
/// window's coherent gain, taken from the table itself rather than assumed.
#[must_use]
pub fn coherent_gain_scale(window: &[f32]) -> f32 {
    let sum: f32 = window.iter().sum();
    if sum > 0.0 { 2.0 / sum } else { 0.0 }
}

/// Split the usable spectrum into `bands` geometric bin ranges, low to high.
///
/// Edges run from [`MIN_HZ`] to Nyquist by a constant ratio. Because the bottom
/// edges are far closer together than one bin is wide — at 44.1 kHz a
/// 2048-point bin spans ~21.5 Hz, wider than the first several bands — each
/// range is forced at least one bin past the last. The result is therefore
/// contiguous, monotonic and non-overlapping, and never reaches past the Nyquist
/// bin.
///
/// Given more bands than the transform has bins to spare, the ranges that run
/// off the end come back empty and read as silence. Degenerate arguments (no
/// bands, a transform too small to have a spectrum, a sample rate that isn't a
/// positive number) answer with an empty map.
#[must_use]
pub fn band_bins(bands: usize, fft_size: usize, sample_rate: f32) -> Box<[Range<usize>]> {
    let nyquist = sample_rate / 2.0;
    if bands == 0 || fft_size < 2 || !sample_rate.is_finite() || nyquist <= MIN_HZ {
        return Box::default();
    }

    let bin_count = fft_size / 2 + 1;
    let max_bin = bin_count - 1;
    let ratio = nyquist / MIN_HZ;
    let bands_f = index_to_f32(bands);

    // Bin 0 is DC, which no band should ever show.
    let mut lo = hz_to_bin(MIN_HZ, fft_size, sample_rate, max_bin).max(1);
    let mut map = Vec::with_capacity(bands);
    for k in 1..=bands {
        let edge_hz = MIN_HZ * ratio.powf(index_to_f32(k) / bands_f);
        // `+ 1` because the edge's own bin belongs to this band; the two `max`es
        // keep the range non-empty, and the last one keeps it valid once `lo`
        // has run past the end of the spectrum.
        let hi = (hz_to_bin(edge_hz, fft_size, sample_rate, max_bin) + 1)
            .max(lo + 1)
            .min(bin_count)
            .max(lo);
        map.push(lo..hi);
        lo = hi;
    }
    map.into_boxed_slice()
}

/// Compress a scaled linear magnitude to a 0..1 bar height on a decibel scale.
///
/// Silence has no decibel value, so it short-circuits before the logarithm —
/// which is also the only way `-inf` could reach the bars.
#[must_use]
pub fn level_from_magnitude(magnitude: f32) -> f32 {
    if magnitude <= 0.0 {
        return 0.0;
    }
    ((linear_to_db(magnitude) - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// Fold `spectrum` into one 0..1 level per band of `map`, written to `out`.
///
/// Each band takes its **loudest** bin rather than the mean: the high bands span
/// hundreds of bins, and averaging a peak against all the empty ones either side
/// of it leaves the top of the display looking dead.
pub fn bands_from_spectrum(
    spectrum: &[Complex<f32>],
    map: &[Range<usize>],
    scale: f32,
    out: &mut [f32],
) {
    for (slot, range) in out.iter_mut().zip(map) {
        let peak = spectrum
            .get(range.clone())
            .map_or(0.0, |bins| bins.iter().map(|bin| bin.norm()).fold(0.0, f32::max));
        *slot = level_from_magnitude(peak * scale);
    }
    // Bands the map doesn't reach (a shorter map than output) read as silence.
    if let Some(rest) = out.get_mut(map.len()..) {
        rest.fill(0.0);
    }
}

/// Advance the displayed `levels` one frame toward `next`.
///
/// Peak-follow: a level at or below its band's new value jumps to it (scaled by
/// `attack`, where `0.0` is instant), and one above it decays by `decay` per
/// frame but never falls below the band's actual level. Both coefficients are
/// fractions in 0..1.
pub fn smooth(levels: &mut [f32], next: &[f32], attack: f32, decay: f32) {
    for (level, &target) in levels.iter_mut().zip(next) {
        *level = if target > *level {
            attack.mul_add(*level - target, target)
        } else {
            (*level * decay).max(target)
        };
    }
}

// --- the analyzer ------------------------------------------------------------

/// Owns everything the per-frame analysis must not rebuild: the FFT plan and its
/// buffers, the Hann table and its scale, the bin→band map, and the smoothed
/// levels carried between frames.
///
/// Usage is two calls per frame — fill [`window_mut`](Self::window_mut) with the
/// most recent samples, then [`analyze`](Self::analyze). Handing out the FFT's
/// own input buffer keeps the sample copy zero-cost: the ring snapshots straight
/// into the buffer the transform reads.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    /// The analysis window. Filled by the caller, then windowed and **consumed**
    /// by the transform — `realfft` uses it as scratch, so nothing may be read
    /// back out of it afterwards.
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    window: Box<[f32]>,
    scale: f32,
    /// Bin ranges, and the sample rate they were built for. Band edges depend on
    /// the rate, which changes from track to track but not from frame to frame.
    map: Box<[Range<usize>]>,
    mapped_rate: u32,
    /// This frame's band levels, and the smoothed ones the caller draws.
    raw: Box<[f32]>,
    levels: Box<[f32]>,
}

impl SpectrumAnalyzer {
    /// Build an analyzer for a given transform size and bar count, allocating
    /// everything it will ever need. `fft_size` should be an even number ≥ 2 —
    /// [`FFT_SIZE`] in production, where a power of two also buys the planner's
    /// fastest path.
    #[must_use]
    pub fn new(fft_size: usize, bands: usize) -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(fft_size);
        let window = hann_window(fft_size);
        Self {
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            scale: coherent_gain_scale(&window),
            window,
            fft,
            map: Box::default(),
            mapped_rate: 0,
            raw: vec![0.0; bands].into_boxed_slice(),
            levels: vec![0.0; bands].into_boxed_slice(),
        }
    }

    /// The window the next [`analyze`](Self::analyze) call will transform. Fill
    /// it with the most recent samples, oldest first — its previous contents are
    /// spent scratch, not history.
    pub fn window_mut(&mut self) -> &mut [f32] {
        &mut self.input
    }

    /// Analyse the filled window and return the smoothed 0..1 band levels.
    ///
    /// `sample_rate` is the rate the samples were captured at; `0` means nothing
    /// has played yet, in which case there is no spectrum to compute and the
    /// bars simply decay. The band map is rebuilt only when the rate changes.
    pub fn analyze(&mut self, sample_rate: u32) -> &[f32] {
        if sample_rate == 0 {
            self.raw.fill(0.0);
        } else {
            if sample_rate != self.mapped_rate {
                self.map = band_bins(self.raw.len(), self.input.len(), rate_to_f32(sample_rate));
                self.mapped_rate = sample_rate;
            }
            for (sample, weight) in self.input.iter_mut().zip(&self.window) {
                *sample *= weight;
            }
            let transformed = self
                .fft
                .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
                .is_ok();
            if transformed {
                bands_from_spectrum(&self.spectrum, &self.map, self.scale, &mut self.raw);
            } else {
                // Unreachable: every buffer came from this plan, so their sizes
                // agree by construction. Decaying beats panicking on the UI thread.
                self.raw.fill(0.0);
            }
        }
        smooth(&mut self.levels, &self.raw, ATTACK, DECAY);
        &self.levels
    }
}

#[cfg(test)]
#[path = "tests/spectrum_tests.rs"]
mod tests;
