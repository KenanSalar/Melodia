//! Spectrum analysis for the audio visualizer: a window of raw samples off the ring, into the
//! per-band bar heights the UI draws.
//!
//! Pure DSP — nothing here knows about the output layer, Slint or threads, and every step is a
//! free function over slices, so the maths is unit-tested directly rather than through a running
//! player. Hann window, two real FFTs (no single window serves both ends), [`NUM_BANDS`] geometric
//! bands read off the bins, a tilt, a decibel compression, then peak-follow smoothing.
//!
//! [`SpectrumAnalyzer`] is the only stateful piece, holding what must not be rebuilt per frame.
//! Nothing in the analysis path allocates.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use super::dsp::{VISUALIZER_DECAY, db_to_linear, index_to_f32, linear_to_db};

/// Samples per analysis window. At 44.1 kHz this is ~46 ms — long enough to resolve bass, short
/// enough to keep the bars in step with what you hear. A power of two hits the FFT planner's
/// fastest path.
pub const FFT_SIZE: usize = 2048;

/// Samples per analysis window for the bands below the crossover. ~186 ms at 44.1 kHz — far too
/// laggy for treble, but the only way to separate bars a few Hz apart, and bass is slow enough
/// not to mind. CAVA pairs the same two sizes.
pub const BASS_FFT_SIZE: usize = 8192;

/// Bars drawn across the display.
pub const NUM_BANDS: usize = 64;

/// Bottom edge of the lowest band. Below this is inaudible rumble and, at bin 0, any DC offset.
/// 50 rather than 20 Hz because a 20 Hz bar is near-always silent *and* wider than an octave on
/// its own at this transform size. CAVA and `DeaDBeeF` settle on the same floor.
const MIN_HZ: f32 = 50.0;

/// Top edge of the highest band, before the Nyquist clamp. Matches the equalizer's top ISO octave
/// band ([`BAND_FREQS`]), so the two features describe one range and the top bar answers the top
/// slider. Every lossy codec lowpasses at or below it, so a band above plots the encoder's
/// silence.
///
/// [`BAND_FREQS`]: super::equalizer::BAND_FREQS
const MAX_HZ: f32 = 16_000.0;

/// Empty-bar level. With [`CEILING_DB`] it sets how much quiet detail shows.
const FLOOR_DB: f32 = -75.0;

/// Full-bar level. Below full scale on purpose — only a lone bass tone gets near 0 dBFS, so a
/// ceiling there wastes the top of every bar's travel. No shipping analyzer uses full scale.
const CEILING_DB: f32 = -15.0;

/// Decibels added per octave above [`TILT_PIVOT_HZ`], subtracted below it. Music is roughly pink,
/// so an untilted display leans bass.
///
/// **The correction on top of the one [`band_magnitude`] already applies**, and so smaller than
/// the 3–5 dB/octave analyzers usually quote: RSS across a band whose width grows with frequency
/// is itself ~3 dB/octave, and it is the *total* that has to land beside CAVA's 5.1.
const TILT_DB_PER_OCTAVE: f32 = 2.0;

/// The one frequency the tilt leaves alone.
const TILT_PIVOT_HZ: f32 = 1000.0;

/// Fraction of the remaining distance a bar *keeps* when rising: `0.0` snaps straight to a new
/// peak, which is what makes a transient read as a hit.
const ATTACK: f32 = 0.0;

// --- casts -------------------------------------------------------------------

#[expect(
    clippy::cast_precision_loss,
    reason = "audio sample rates are well below 2^24 Hz and convert to f32 exactly"
)]
fn rate_to_f32(hz: u32) -> f32 {
    hz as f32
}

/// The FFT bin a frequency falls in — **fractional**, because a band edge rarely lands on a whole
/// bin and rounding it is exactly what this module must not do.
fn hz_to_bin(hz: f32, fft_size: usize, sample_rate: f32) -> f32 {
    hz * index_to_f32(fft_size) / sample_rate
}

/// The frequency a — possibly fractional — bin sits at: the inverse of [`hz_to_bin`].
fn bin_to_hz(bin: f32, fft_size: usize, sample_rate: f32) -> f32 {
    bin * sample_rate / index_to_f32(fft_size)
}

/// A bin position as an index. Callers clamp into `1.0..=max_bin` and every read goes through
/// `get`, so a bad cast can't reach past the spectrum.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bin positions are clamped into 1.0..=max_bin before the cast, so they are small and non-negative"
)]
fn bin_to_index(bin: f32) -> usize {
    bin as usize
}

// --- pure DSP ----------------------------------------------------------------

/// A Hann window of `size` points, cutting the spectral leakage a rectangular window smears
/// across neighbouring bands. Built once and reused — it depends on nothing but its length.
#[must_use]
pub fn hann_window(size: usize) -> Box<[f32]> {
    // A one-point window has no span to taper across, and the formula would divide by zero.
    if size < 2 {
        return vec![1.0; size].into_boxed_slice();
    }
    let denom = index_to_f32(size - 1);
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * index_to_f32(i) / denom).cos()))
        .collect()
}

/// The magnitude scale that puts a full-scale sine at 1.0: a windowed sine of amplitude `A` peaks
/// at `A/2 · Σw`, so half the window's sum is the divisor — its coherent gain, read off the table
/// rather than assumed.
#[must_use]
pub fn coherent_gain_scale(window: &[f32]) -> f32 {
    let sum: f32 = window.iter().sum();
    if sum > 0.0 { 2.0 / sum } else { 0.0 }
}

/// The `bands + 1` band boundaries, as **fractional** bin positions, ascending.
///
/// Geometric because pitch perception is, running [`MIN_HZ`] to [`MAX_HZ`] with the top clamped
/// to Nyquist — a 22 kHz-sampled file has no 16 kHz to show, and edges past its spectrum would
/// pile the last several bands onto one bin.
///
/// Deliberately **not** rounded to whole bins: a bin is wider than any of the bottom bands, so
/// snapping each to its own would march the low end up the spectrum one bin per bar, giving a
/// linear ramp whose bars span wildly uneven slices of an octave. Fractional edges let
/// [`bands_from_spectrum`] interpolate down there — Audacious's and `DeaDBeeF`'s choice.
///
/// Every edge lands in `1.0..=max_bin`, bin 0 being DC. Degenerate arguments answer with no edges
/// at all.
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

/// Compress a scaled linear magnitude to a 0..1 bar height, [`FLOOR_DB`] to [`CEILING_DB`] across
/// the bar. Silence short-circuits before the logarithm, which is also the only way `-inf` could
/// reach the bars.
#[must_use]
pub fn level_from_magnitude(magnitude: f32) -> f32 {
    if magnitude <= 0.0 {
        return 0.0;
    }
    ((linear_to_db(magnitude) - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// One linear gain per band of `edges`, tilting [`TILT_DB_PER_OCTAVE`] about [`TILT_PIVOT_HZ`] at
/// each band's **geometric** centre — its true midpoint on the log axis the bands sit on. Depends
/// only on the band map, so it is built alongside it and reused every frame.
#[must_use]
pub fn band_tilt_gains(edges: &[f32], fft_size: usize, sample_rate: f32) -> Box<[f32]> {
    edges
        .iter()
        .zip(edges.iter().skip(1))
        .map(|(&lo, &hi)| {
            let lo_hz = bin_to_hz(lo, fft_size, sample_rate);
            let hi_hz = bin_to_hz(hi, fft_size, sample_rate);
            db_to_linear(TILT_DB_PER_OCTAVE * ((lo_hz * hi_hz).sqrt() / TILT_PIVOT_HZ).log2())
        })
        .collect()
}

/// The magnitude a band spanning `lo..hi` reads: the **root-sum-square** of the whole bins it
/// contains, or, for a band narrower than one bin, the spectrum interpolated at its centre.
///
/// Power-domain summing is load-bearing. A **peak** runs high wherever a band holds many bins,
/// favouring tonal content over broadband, which pins the bass and kills the treble; a **mean**
/// overcorrects, dividing a lone tone by its band's width. RSS does neither, growing as `√bins`
/// for broadband. The narrow case exists at all only because the edges stay fractional.
fn band_magnitude(spectrum: &[Complex<f32>], lo: f32, hi: f32) -> f32 {
    let first = lo.ceil();
    let last = hi.floor();

    if last >= first {
        let bins = bin_to_index(first)..=bin_to_index(last);
        return spectrum
            .get(bins)
            .map_or(0.0, |bins| bins.iter().map(Complex::norm_sqr).sum::<f32>().sqrt());
    }

    let centre = 0.5 * (lo + hi);
    let below = centre.floor();
    let toward_next = centre - below;
    let index = bin_to_index(below);
    let near = spectrum.get(index).map_or(0.0, |bin| bin.norm());
    // No bin above means the centre sits on the last one; hold it rather than interpolating
    // toward a zero that isn't there.
    let far = spectrum.get(index + 1).map_or(near, |bin| bin.norm());
    toward_next.mul_add(far - near, near)
}

/// Fold `spectrum` into one 0..1 level per band of `edges`, written to `out`. `edges` holds one
/// entry more than there are bands; `gains` one per band.
pub fn bands_from_spectrum(
    spectrum: &[Complex<f32>],
    edges: &[f32],
    gains: &[f32],
    scale: f32,
    out: &mut [f32],
) {
    let pairs = edges.iter().zip(edges.iter().skip(1));
    let mut bands = 0;
    for ((slot, (&lo, &hi)), &gain) in out.iter_mut().zip(pairs).zip(gains) {
        *slot = level_from_magnitude(band_magnitude(spectrum, lo, hi) * scale * gain);
        bands += 1;
    }
    // Bands no edge pair or gain reached read as silence.
    if let Some(rest) = out.get_mut(bands..) {
        rest.fill(0.0);
    }
}

/// Advance the displayed `levels` one frame toward `next`, peak-follow: rise at once (scaled by
/// `attack`, `0.0` being instant), fall by `decay` per frame but never below the band's actual
/// level. So the bars read lively, not twitchy.
pub fn smooth(levels: &mut [f32], next: &[f32], attack: f32, decay: f32) {
    for (level, &target) in levels.iter_mut().zip(next) {
        *level = if target > *level {
            attack.mul_add(*level - target, target)
        } else {
            (*level * decay).max(target)
        };
    }
}

/// The first band the main transform resolves — the first one a whole bin wide. Everything below
/// it is read from the bass transform instead.
///
/// **Width, not "contains a whole bin".** The latter is what [`band_magnitude`] branches on but
/// isn't monotonic down here: a band far narrower than a bin still contains one whenever it
/// straddles an integer, so the first match lands several bands below where the display stops
/// being resolved. Width is monotonic, the bands being geometric.
///
/// Derived rather than fixed in Hz, so a 96 kHz file's wider bins move it up. With no band
/// resolved, every band belongs to the bass transform.
#[must_use]
pub fn crossover_band(main_edges: &[f32]) -> usize {
    main_edges
        .iter()
        .zip(main_edges.iter().skip(1))
        .position(|(&lo, &hi)| hi - lo >= 1.0)
        .unwrap_or_else(|| main_edges.len().saturating_sub(1))
}

// --- the analyzer ------------------------------------------------------------

/// One windowed real transform, plus the band map for the bins it resolves.
struct Transform {
    fft: Arc<dyn RealToComplex<f32>>,
    /// The analysis window. Filled by the caller, then windowed and **consumed** by the
    /// transform — `realfft` uses it as scratch, so nothing may be read back out of it after.
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    window: Box<[f32]>,
    scale: f32,
    edges: Box<[f32]>,
}

impl Transform {
    /// Takes the planner rather than building one, so both transforms share its caches:
    /// `realfft`'s plan cache sits over a `rustfft` planner caching the base butterflies a
    /// radix-4 decomposition pulls from, and both our sizes are powers of two.
    fn new(planner: &mut RealFftPlanner<f32>, size: usize) -> Self {
        let fft = planner.plan_fft_forward(size);
        let window = hann_window(size);
        Self {
            input: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            scale: coherent_gain_scale(&window),
            window,
            fft,
            edges: Box::default(),
        }
    }

    fn remap(&mut self, bands: usize, rate: f32) {
        self.edges = band_edges(bands, self.input.len(), rate);
    }

    /// Window and transform the filled input. `false` only if the plan rejected its own buffers,
    /// which their construction rules out.
    fn run(&mut self) -> bool {
        for (sample, weight) in self.input.iter_mut().zip(&self.window) {
            *sample *= weight;
        }
        self.fft
            .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
            .is_ok()
    }
}

/// Everything the per-frame analysis must not rebuild: the two FFT plans and their buffers, the
/// Hann tables and scales, the bin→band maps with their tilt gains, and the smoothed levels
/// carried between frames.
///
/// Two calls per frame — [`fill_windows`](Self::fill_windows), then [`analyze`](Self::analyze).
///
/// **Two transforms, because one can't serve both ends.** A window short enough to keep treble
/// responsive leaves the lowest bands — a few Hz wide against a bin that isn't — interpolating
/// the same handful of bins and moving as one block, while a window long enough to separate them
/// smears every cymbal. So the bass reads [`BASS_FFT_SIZE`] and everything above the crossover
/// reads [`FFT_SIZE`], as CAVA does.
///
/// The join needs no correction factor, which is a property of the RSS in [`band_magnitude`]:
/// per-bin magnitude falls as `1/√N` under [`coherent_gain_scale`] while a band of fixed width
/// holds `∝ N` bins, so a band reads the same either side.
pub struct SpectrumAnalyzer {
    bass: Transform,
    main: Transform,
    /// First band read from `main`; everything below it comes from `bass`.
    crossover: usize,
    /// Per-band tilt gains, and the sample rate every map above was built for.
    gains: Box<[f32]>,
    mapped_rate: u32,
    /// This frame's band levels, and the smoothed ones the caller draws.
    raw: Box<[f32]>,
    levels: Box<[f32]>,
}

impl SpectrumAnalyzer {
    /// Allocates everything the analyzer will ever need. `fft_size` must be an even number ≥ 2 —
    /// `realfft` plans an odd length on a separate half-length path, and the bin maths here reads
    /// the even layout.
    #[must_use]
    pub fn new(fft_size: usize, bands: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        Self {
            bass: Transform::new(&mut planner, BASS_FFT_SIZE),
            main: Transform::new(&mut planner, fft_size),
            crossover: 0,
            gains: Box::default(),
            mapped_rate: 0,
            raw: vec![0.0; bands].into_boxed_slice(),
            levels: vec![0.0; bands].into_boxed_slice(),
        }
    }

    /// The two windows the next [`analyze`](Self::analyze) will transform, bass first. Fill both
    /// with the most recent samples, newest last — their previous contents are spent scratch, not
    /// history.
    pub fn windows_mut(&mut self) -> (&mut [f32], &mut [f32]) {
        (&mut self.bass.input, &mut self.main.input)
    }

    /// Fill both windows from a single read of the sample source.
    ///
    /// `read` runs **once**: the short window is the newest tail of the long one, so copying it
    /// across is cheaper than a second read *and* more correct — two reads land a few samples
    /// apart and leave the transforms analysing different instants. The fallback exists only for
    /// a main window longer than the bass one, which production never builds.
    pub fn fill_windows(&mut self, read: impl Fn(&mut [f32])) {
        let (bass, main) = self.windows_mut();
        // Reborrowed, not handed over — `bass` is read again below.
        read(&mut *bass);
        match bass.len().checked_sub(main.len()) {
            Some(tail) => main.copy_from_slice(&bass[tail..]),
            None => read(main),
        }
    }

    /// Analyse the filled windows and return the smoothed 0..1 band levels. `sample_rate` of `0`
    /// means nothing has played yet, so the bars decay.
    pub fn analyze(&mut self, sample_rate: u32) -> &[f32] {
        if sample_rate == 0 {
            self.raw.fill(0.0);
        } else {
            self.remap_for_rate(sample_rate);
            // Unreachable otherwise: every buffer came from its own plan, so their sizes agree.
            // Decaying beats panicking on the UI thread.
            if self.bass.run() && self.main.run() {
                self.fold_bands();
            } else {
                self.raw.fill(0.0);
            }
        }
        smooth(&mut self.levels, &self.raw, ATTACK, VISUALIZER_DECAY);
        &self.levels
    }

    /// Rebuild the band edges, crossover and tilt gains for `sample_rate`. A no-op unless the
    /// rate changed, which is once per track at most.
    fn remap_for_rate(&mut self, sample_rate: u32) {
        if sample_rate == self.mapped_rate {
            return;
        }
        let rate = rate_to_f32(sample_rate);
        let bands = self.raw.len();
        self.bass.remap(bands, rate);
        self.main.remap(bands, rate);
        self.crossover = crossover_band(&self.main.edges);
        self.gains = band_tilt_gains(&self.main.edges, self.main.input.len(), rate);
        self.mapped_rate = sample_rate;
    }

    /// Read each half of the display off the transform that resolves it.
    fn fold_bands(&mut self) {
        let split = self.crossover.min(self.raw.len());
        let (low, high) = self.raw.split_at_mut(split);
        // `edges` carries one more entry than bands, so both slices keep the boundary they share.
        bands_from_spectrum(
            &self.bass.spectrum,
            self.bass.edges.get(..=split).unwrap_or_default(),
            self.gains.get(..split).unwrap_or_default(),
            self.bass.scale,
            low,
        );
        bands_from_spectrum(
            &self.main.spectrum,
            self.main.edges.get(split..).unwrap_or_default(),
            self.gains.get(split..).unwrap_or_default(),
            self.main.scale,
            high,
        );
    }
}

#[cfg(test)]
#[path = "tests/spectrum_tests.rs"]
mod tests;
