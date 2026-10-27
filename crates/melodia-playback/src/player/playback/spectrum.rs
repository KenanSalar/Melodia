//! The visualizer's spectrum: a window off the ring into the per-band bar heights the UI draws.
//! Pure DSP over slices, unit-tested directly: a Hann window, two real FFTs (no one window serves
//! both ends), [`NUM_BANDS`] geometric bands, a tilt, a decibel compression, then peak-follow
//! smoothing. Nothing in the analysis path allocates.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use super::dsp::{VISUALIZER_DECAY, db_to_linear, index_to_f32, linear_to_db, push_fixed};

/// ~46 ms at 44.1 kHz: long enough for bass, short enough to stay in step with what you hear.
pub const FFT_SIZE: usize = 2048;

/// ~186 ms at 44.1 kHz, too laggy for treble but the only way to separate bass bars a few Hz
/// apart.
pub const BASS_FFT_SIZE: usize = 8192;

/// Bars drawn across the display.
pub const NUM_BANDS: usize = 64;

/// Below is rumble and DC; a 20 Hz bar is near-always silent and wider than an octave here.
const MIN_HZ: f32 = 50.0;

/// The equalizer's top ISO band ([`BAND_FREQS`]), so the top bar answers the top slider. Every
/// lossy codec lowpasses at or below it.
///
/// [`BAND_FREQS`]: super::equalizer::BAND_FREQS
const MAX_HZ: f32 = 16_000.0;

/// Empty-bar level. With [`CEILING_DB`] it sets how much quiet detail shows.
const FLOOR_DB: f32 = -75.0;

/// Below full scale, which only a lone bass tone nears, or the top of every bar's travel is waste.
const CEILING_DB: f32 = -15.0;

/// Music is roughly pink, so an untilted display leans bass. Small because [`band_magnitude`]'s
/// RSS already tilts ~3 dB/octave, and the total is what has to land (`audio-stack.md`).
const TILT_DB_PER_OCTAVE: f32 = 2.0;

/// The one frequency the tilt leaves alone.
const TILT_PIVOT_HZ: f32 = 1000.0;

/// Fraction of the remaining distance a bar *keeps* when rising: `0.0` snaps straight to a new
/// peak, which is what makes a transient read as a hit.
const ATTACK: f32 = 0.0;

#[expect(
    clippy::cast_precision_loss,
    reason = "audio sample rates are well below 2^24 Hz and convert to f32 exactly"
)]
fn rate_to_f32(hz: u32) -> f32 {
    hz as f32
}

/// Fractional: a band edge rarely lands on a whole bin, and rounding it is what this module must
/// not do.
fn hz_to_bin(hz: f32, fft_size: usize, sample_rate: f32) -> f32 {
    hz * index_to_f32(fft_size) / sample_rate
}

fn bin_to_hz(bin: f32, fft_size: usize, sample_rate: f32) -> f32 {
    bin * sample_rate / index_to_f32(fft_size)
}

/// Callers clamp first and every read goes through `get`, so a bad cast can't reach past the
/// spectrum.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "bin positions are clamped into 1.0..=max_bin before the cast, so they are small and non-negative"
)]
fn bin_to_index(bin: f32) -> usize {
    bin as usize
}

/// Cuts the leakage a rectangular window smears across neighbouring bands; built once and reused.
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

/// The `bands + 1` edges as fractional bins, geometric from [`MIN_HZ`] to [`MAX_HZ`] clamped to
/// Nyquist, inside `1.0..=max_bin`. Not rounded: a bin is wider than any bottom band, and snapping
/// would march the low end up one bin per bar, so [`bands_from_spectrum`] interpolates there.
/// Degenerate arguments answer with no edges.
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

/// [`FLOOR_DB`] to [`CEILING_DB`] across the bar. Silence short-circuits before the logarithm,
/// the only way `-inf` could reach the bars.
#[must_use]
pub fn level_from_magnitude(magnitude: f32) -> f32 {
    if magnitude <= 0.0 {
        return 0.0;
    }
    ((linear_to_db(magnitude) - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// The tilt at each band's geometric centre, its midpoint on the log axis; built with the band map.
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

/// The root-sum-square of the band's whole bins, or the spectrum interpolated at its centre for a
/// band under a bin wide. Not a peak, which favours tones and kills the treble, nor a mean, which
/// divides a lone tone by its band's width.
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

/// Peak-follow: rise at once (`attack` `0.0`), fall by `decay` per frame but never below the
/// band's level, so the bars read lively rather than twitchy.
pub fn smooth(levels: &mut [f32], next: &[f32], attack: f32, decay: f32) {
    for (level, &target) in levels.iter_mut().zip(next) {
        *level = if target > *level {
            attack.mul_add(*level - target, target)
        } else {
            (*level * decay).max(target)
        };
    }
}

/// The first band a whole bin wide; everything below reads the bass transform. Width rather than
/// "contains a bin", which a narrow band straddling an integer passes, so isn't monotonic.
/// Derived, so a 96 kHz file's wider bins move it up.
#[must_use]
pub fn crossover_band(main_edges: &[f32]) -> usize {
    main_edges
        .iter()
        .zip(main_edges.iter().skip(1))
        .position(|(&lo, &hi)| hi - lo >= 1.0)
        .unwrap_or_else(|| main_edges.len().saturating_sub(1))
}

/// One windowed real transform, plus the band map for the bins it resolves.
struct Transform {
    fft: Arc<dyn RealToComplex<f32>>,
    /// Consumed as scratch by the transform, so never read back.
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    window: Box<[f32]>,
    scale: f32,
    edges: Box<[f32]>,
}

impl Transform {
    /// Shares the planner, so both power-of-two sizes share its butterfly caches.
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

    /// `false` only if the plan rejected its own buffers.
    fn run(&mut self) -> bool {
        for (sample, weight) in self.input.iter_mut().zip(&self.window) {
            *sample *= weight;
        }
        self.fft
            .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
            .is_ok()
    }
}

/// What the per-frame analysis must not rebuild; [`fill_windows`](Self::fill_windows), then
/// [`analyze`](Self::analyze). Two transforms, because a window short enough for treble leaves the
/// bottom bands moving as one block and one long enough to separate them smears every cymbal. The
/// join needs no correction: per-bin magnitude falls as `1/√N` while a band holds `∝ N` bins.
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
    /// `fft_size` must be even and ≥ 2: `realfft` plans an odd length on another path, and the bin
    /// maths here reads the even layout.
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

    /// Bass first, to be filled newest last; their old contents are spent scratch.
    pub fn windows_mut(&mut self) -> (&mut [f32], &mut [f32]) {
        (&mut self.bass.input, &mut self.main.input)
    }

    /// `read` runs once and the short window copies the long one's tail, so both transforms
    /// analyse one instant rather than two reads a few samples apart.
    pub fn fill_windows(&mut self, read: impl Fn(&mut [f32])) {
        let (bass, main) = self.windows_mut();
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

    /// The levels the last [`Self::analyze`] left, for drawing them again without advancing a
    /// frame.
    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    /// A no-op unless the rate changed, once a track at most.
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

/// Where a bar grows from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarAnchor {
    /// Up from the strip's bottom edge.
    Baseline,
    /// Both ways from its centre line, which is the "Mirrored" style.
    Centre,
}

/// The strip in logical pixels, against which the bar gap, the silent floor and the pixel grid are
/// measured.
#[derive(Clone, Copy, Debug)]
pub struct StripGeometry {
    /// From the window's left edge.
    pub x: f32,
    /// From the window's top edge.
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Device pixels per logical one, [`write_bar_path`] laying edges on whole device pixels.
    pub scale: f32,
}

/// Tight, or at [`NUM_BANDS`] a wider gap eats the bars on a narrow window.
const BAR_GAP_PX: f32 = 2.0;

/// Enough that a normalized edge round-trips nearer its pixel than the half-pixel either side, or
/// a bar changes width; the most [`push_fixed`]'s `u16` scale carries.
const COORD_DECIMALS: u32 = 4;

/// One axis on the window's device-pixel grid. The renderer rounds neither origin nor scale, and
/// with anti-aliasing off an edge on a pixel centre lets float noise pick its side; laid out
/// between grid lines, every edge is half a pixel clear.
struct GridAxis {
    /// How far past a grid line the strip starts, in device pixels.
    phase: f32,
    /// The first grid line inside the strip.
    first: f32,
    /// The last grid line inside the strip.
    last: f32,
    /// The strip's device-pixel length, which the viewbox normalizes against.
    extent: f32,
}

impl GridAxis {
    fn new(origin: f32, length: f32, scale: f32) -> Self {
        let phase = (origin * scale).rem_euclid(1.0);
        let extent = length * scale;
        Self { phase, first: phase.ceil(), last: (phase + extent).floor(), extent }
    }

    /// Whole device pixels between the first grid line and the last.
    fn pixels(&self) -> f32 {
        self.last - self.first
    }

    /// Whether there is a whole pixel to draw into. False for a strip no layout pass has sized
    /// yet, and for any geometry that isn't a number.
    fn is_drawable(&self) -> bool {
        self.pixels().is_finite() && self.pixels() >= 1.0
    }

    /// A grid line as a fraction of the strip, which `fit: fill` maps back onto that line.
    fn normalize(&self, line: f32) -> f32 {
        (line - self.phase) / self.extent
    }
}

/// `levels` as one SVG figure of closed rectangles, normalized to a constant viewbox so the band
/// count never crosses into Slint, and empty while the strip has fewer device pixels than bands.
/// One figure because `FemtoVG` tessellates a path per element per frame, caching none.
pub fn write_bar_path(levels: &[f32], strip: StripGeometry, anchor: BarAnchor, out: &mut String) {
    out.clear();
    if levels.is_empty() {
        return;
    }
    let count = index_to_f32(levels.len());

    // A strip not yet laid out draws nothing; the pass that sizes it asks again.
    let horizontal = GridAxis::new(strip.x, strip.width, strip.scale);
    let vertical = GridAxis::new(strip.y, strip.height, strip.scale);
    if !horizontal.is_drawable() || !vertical.is_drawable() {
        return;
    }

    // A whole pitch, so every bar and every gap match; the remainder goes to the two ends.
    let Some((pitch, gap)) = band_pitch(horizontal.pixels(), count, BAR_GAP_PX * strip.scale)
    else {
        return;
    };
    let bar_width = pitch - gap;
    let span = pitch * count - gap;
    let origin = horizontal.first + ((horizontal.pixels() - span) * 0.5).round();
    // A silent band rests as a square mark rather than vanishing.
    let floor = bar_width.min(vertical.pixels());
    let middle = (vertical.first + vertical.last) * 0.5;

    for (i, &level) in levels.iter().enumerate() {
        let height = (level.clamp(0.0, 1.0) * vertical.pixels()).max(floor);
        let (top, bottom) = match anchor {
            BarAnchor::Baseline => (vertical.last - height, vertical.last),
            // The total in both anchorings, half either side of the axis.
            BarAnchor::Centre => (middle - height * 0.5, middle + height * 0.5),
        };
        let top = top.round();
        // A band at the floor can round both edges onto one row, which would draw nothing at all.
        let bottom = bottom.round().max(top + 1.0);
        let left = origin + index_to_f32(i) * pitch;
        let right = left + bar_width;

        let (left, right) = (horizontal.normalize(left), horizontal.normalize(right));
        let (top, bottom) = (vertical.normalize(top), vertical.normalize(bottom));
        // Lower edge first, `waveform`'s winding, femtovg taking a negative area for a hole. `H`
        // and `V` keep a string formatted every tick and parsed every frame short.
        push_vertex(out, "M", left, bottom);
        push_coordinate(out, " H", right);
        push_coordinate(out, " V", top);
        push_coordinate(out, " H", left);
        out.push('Z');
    }
}

/// The widest whole-pixel pitch whose last band ends inside the strip, `None` with fewer pixels
/// than bands. The gap is held to half the pitch and dropped below two pixels a band, or a narrow
/// strip degrades into hairlines; the pitch is re-checked against the gap it got.
fn band_pitch(pixels: f32, count: f32, preferred_gap: f32) -> Option<(f32, f32)> {
    if pixels < count {
        return None;
    }
    let gap_at = |pitch: f32| {
        if pitch < 2.0 { 0.0 } else { preferred_gap.round().clamp(1.0, (pitch * 0.5).floor()) }
    };
    let mut pitch = ((pixels + preferred_gap.round().max(1.0)) / count).floor();
    while pitch * count - gap_at(pitch) > pixels {
        pitch -= 1.0;
    }
    Some((pitch, gap_at(pitch)))
}

/// One `"<cmd><x> <y>"` vertex.
fn push_vertex(out: &mut String, command: &str, x: f32, y: f32) {
    push_coordinate(out, command, x);
    out.push(' ');
    push_fixed::<COORD_DECIMALS>(out, y);
}

/// One `"<cmd><value>"` command taking a single coordinate.
fn push_coordinate(out: &mut String, command: &str, value: f32) {
    out.push_str(command);
    push_fixed::<COORD_DECIMALS>(out, value);
}

#[cfg(test)]
#[path = "tests/spectrum_tests.rs"]
mod tests;
