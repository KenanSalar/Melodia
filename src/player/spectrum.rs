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
//! 3. read [`NUM_BANDS`] **geometric** bands off those bins — pitch perception is
//!    geometric, so linear bands would spend most of the display on treble,
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

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use super::dsp::linear_to_db;

/// Samples per analysis window. At 44.1 kHz this is ~46 ms — long enough to
/// resolve bass, short enough to keep the bars in step with what you hear. A
/// power of two hits the FFT planner's fastest path.
pub const FFT_SIZE: usize = 2048;

/// Bars drawn across the display.
pub const NUM_BANDS: usize = 64;

/// Bottom edge of the lowest band. Below this is inaudible rumble and, at bin 0,
/// any DC offset — neither belongs in a bass bar. 50 Hz rather than 20 because a
/// 20 Hz bar is not only near-always silent, it is also *wider than an octave* on
/// its own at this transform size, so it costs a bar and shows nothing. CAVA and
/// `DeaDBeeF` settle on the same floor.
const MIN_HZ: f32 = 50.0;

/// Top edge of the highest band, before the Nyquist clamp. Matches the
/// equalizer's top ISO octave band ([`BAND_FREQS`]), so the two features describe
/// one frequency range and the top bar answers the top slider. Every lossy codec
/// lowpasses at or below this, so a band above it plots an encoder's silence
/// rather than the track.
///
/// [`BAND_FREQS`]: super::equalizer::BAND_FREQS
const MAX_HZ: f32 = 16_000.0;

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

/// The FFT bin a frequency falls in — **fractional**, because a band edge rarely
/// lands on a whole bin and rounding it is exactly what this module must not do.
fn hz_to_bin(hz: f32, fft_size: usize, sample_rate: f32) -> f32 {
    hz * index_to_f32(fft_size) / sample_rate
}

/// A bin position as an index. Callers pass values already clamped into
/// `1.0..=max_bin`, and every read then goes through `get`, so a bad cast cannot
/// reach past the spectrum.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bin positions are clamped into 1.0..=max_bin before the cast, so they are small and non-negative"
)]
fn bin_to_index(bin: f32) -> usize {
    bin as usize
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

/// The `bands + 1` band boundaries, as **fractional** bin positions, ascending.
///
/// Edges run from [`MIN_HZ`] to [`MAX_HZ`] by a constant ratio, with the top
/// clamped to Nyquist — a 22 kHz-sampled file has no 16 kHz to show, and letting
/// the edges run past its spectrum would pile the last several bands onto one bin.
///
/// They are deliberately **not** rounded to whole bins. At 44.1 kHz a 2048-point
/// bin spans ~21.5 Hz, which is wider than any of the bottom bands, so snapping
/// each to a bin of its own is what would march the low end up the spectrum one
/// bin per bar: a linear ramp whose bars span wildly uneven slices of an octave
/// (the lowest more than a whole one). Fractional edges let
/// [`bands_from_spectrum`] interpolate down there instead — the choice Audacious
/// and `DeaDBeeF` both make, and cheaper than CAVA's second, longer transform for
/// the bass.
///
/// Every edge lands in `1.0..=max_bin`: bin 0 is DC, which belongs to no band.
/// Degenerate arguments (no bands, a transform too small to have a spectrum, a
/// sample rate that isn't a positive number or is too low to reach [`MIN_HZ`])
/// answer with no edges at all.
#[must_use]
pub fn band_edges(bands: usize, fft_size: usize, sample_rate: f32) -> Box<[f32]> {
    let nyquist = sample_rate / 2.0;
    if bands == 0 || fft_size < 2 || !sample_rate.is_finite() || nyquist <= MIN_HZ {
        return Box::default();
    }

    let max_bin = index_to_f32(fft_size / 2);
    let ratio = MAX_HZ.min(nyquist) / MIN_HZ;
    let bands_f = index_to_f32(bands);

    (0..=bands)
        .map(|k| {
            let hz = MIN_HZ * ratio.powf(index_to_f32(k) / bands_f);
            hz_to_bin(hz, fft_size, sample_rate).clamp(1.0, max_bin)
        })
        .collect()
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

/// The magnitude a band spanning `lo..hi` (fractional bin positions) reads.
///
/// A band wide enough to contain whole bins takes its **loudest** one rather than
/// the mean: the high bands span hundreds of bins, and averaging a peak against
/// all the empty ones either side of it leaves the top of the display dead.
///
/// A band narrower than a single bin has no bin of its own to take, so it samples
/// the spectrum *between* the two that bracket its centre. That is the whole point
/// of keeping the edges fractional: without it every band down there would seize
/// the next bin outright and neighbouring bars would step rather than slope.
fn band_magnitude(spectrum: &[Complex<f32>], lo: f32, hi: f32) -> f32 {
    let first = lo.ceil();
    let last = hi.floor();

    if last >= first {
        let bins = bin_to_index(first)..=bin_to_index(last);
        return spectrum
            .get(bins)
            .map_or(0.0, |bins| bins.iter().map(|bin| bin.norm()).fold(0.0, f32::max));
    }

    let centre = 0.5 * (lo + hi);
    let below = centre.floor();
    let toward_next = centre - below;
    let index = bin_to_index(below);
    let near = spectrum.get(index).map_or(0.0, |bin| bin.norm());
    // No bin above means the centre sits on the last one; hold it rather than
    // interpolating toward a zero that isn't there.
    let far = spectrum.get(index + 1).map_or(near, |bin| bin.norm());
    toward_next.mul_add(far - near, near)
}

/// Fold `spectrum` into one 0..1 level per band of `edges`, written to `out`.
///
/// `edges` holds one more entry than there are bands — see [`band_edges`].
pub fn bands_from_spectrum(spectrum: &[Complex<f32>], edges: &[f32], scale: f32, out: &mut [f32]) {
    let pairs = edges.iter().zip(edges.iter().skip(1));
    for (slot, (&lo, &hi)) in out.iter_mut().zip(pairs) {
        *slot = level_from_magnitude(band_magnitude(spectrum, lo, hi) * scale);
    }
    // Bands the edges don't reach (fewer edges than output slots) read as silence.
    if let Some(rest) = out.get_mut(edges.len().saturating_sub(1)..) {
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
    /// Band edges, and the sample rate they were built for. Edges depend on the
    /// rate, which changes from track to track but not from frame to frame.
    edges: Box<[f32]>,
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
            edges: Box::default(),
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
                self.edges = band_edges(self.raw.len(), self.input.len(), rate_to_f32(sample_rate));
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
                bands_from_spectrum(&self.spectrum, &self.edges, self.scale, &mut self.raw);
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
