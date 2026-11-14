//! The cascade's magnitude response at a source's rate, read for its loudest point: a peak sits
//! where curves overlap, so no single filter's gain says how high.

use std::f64::consts::TAU;

use biquad::Coefficients;

const GRID_FLOOR_HZ: f64 = 20.0;
const GRID_CEILING_HZ: f64 = 20_000.0;

/// Coarser lets a peak between two points read low, leaving the limiter to catch it.
const GRID_STEPS_PER_OCTAVE: f64 = 24.0;

/// A frequency as the response formula takes it, built once per point rather than per filter.
#[derive(Clone, Copy, Debug)]
struct Point {
    cos_w: f64,
    cos_2w: f64,
}

/// A biquad's squared magnitude, `|H|²`, as a function of frequency, with the point its design
/// centres on, where a peaking filter's peak sits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FilterResponse {
    numerator: [f64; 3],
    denominator: [f64; 3],
    centre: Option<Point>,
}

impl FilterResponse {
    /// A factor of one at every frequency, with no centre of its own to evaluate.
    pub(crate) const FLAT: Self =
        Self { numerator: [1.0, 0.0, 0.0], denominator: [1.0, 0.0, 0.0], centre: None };

    fn power_at(&self, point: Point) -> f64 {
        let [n0, n1, n2] = self.numerator;
        let [d0, d1, d2] = self.denominator;
        (n0 + n1 * point.cos_w + n2 * point.cos_2w) / (d0 + d1 * point.cos_w + d2 * point.cos_2w)
    }
}

/// The audible band at one source's rate, as fixed log-spaced points.
pub(crate) struct ResponseGrid {
    points: Box<[Point]>,
    sample_rate: f64,
    top_hz: f64,
}

impl ResponseGrid {
    /// Allocates, so it is built with its source on the control thread.
    pub(crate) fn new(sample_rate: f64) -> Self {
        let top_hz = GRID_CEILING_HZ.min(sample_rate / 2.0);
        let step = 2.0_f64.powf(GRID_STEPS_PER_OCTAVE.recip());
        // The steps seldom land on the band edge, where a high shelf's plateau ends.
        let frequencies = std::iter::successors(Some(GRID_FLOOR_HZ), |hz| Some(hz * step))
            .take_while(|&hz| hz < top_hz)
            .chain(std::iter::once(top_hz));
        let points = frequencies.map(|hz| point_at(hz, sample_rate)).collect();
        Self { points, sample_rate, top_hz }
    }

    /// A centre outside the band is read at its nearest edge, so an inaudible peak sets no level.
    pub(crate) fn response(&self, coeffs: &Coefficients<f64>, centre_hz: f64) -> FilterResponse {
        let Coefficients { a1, a2, b0, b1, b2 } = *coeffs;
        FilterResponse {
            numerator: [b0 * b0 + b1 * b1 + b2 * b2, 2.0 * (b0 * b1 + b1 * b2), 2.0 * b0 * b2],
            denominator: [1.0 + a1 * a1 + a2 * a2, 2.0 * (a1 + a1 * a2), 2.0 * a2],
            centre: Some(point_at(centre_hz.max(GRID_FLOOR_HZ).min(self.top_hz), self.sample_rate)),
        }
    }

    /// The linear gain bringing the cascade's loudest point to unity, never above it. Each
    /// filter's `|H|²` is divided out on its own, bounded by its gain, where a product of
    /// numerators over a product of denominators could underflow.
    pub(crate) fn headroom(&self, correction: &[FilterResponse], bands: &[FilterResponse]) -> f64 {
        let cascade = || correction.iter().chain(bands);
        let centres = cascade().filter_map(|filter| filter.centre);
        let peak_power = self
            .points
            .iter()
            .copied()
            .chain(centres)
            .map(|point| cascade().map(|filter| filter.power_at(point)).product::<f64>())
            .fold(0.0, f64::max);
        if peak_power > 1.0 { peak_power.sqrt().recip() } else { 1.0 }
    }
}

fn point_at(hz: f64, sample_rate: f64) -> Point {
    let w = TAU * hz / sample_rate;
    Point { cos_w: w.cos(), cos_2w: (2.0 * w).cos() }
}
