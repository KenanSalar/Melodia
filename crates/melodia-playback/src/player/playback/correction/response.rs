//! The cascade's magnitude response at a source's rate, read for its loudest point: a peak sits
//! where curves overlap, so no single filter's gain says how high.

use std::f64::consts::PI;

use biquad::Coefficients;

const GRID_FLOOR_HZ: f64 = 20.0;
const GRID_CEILING_HZ: f64 = 20_000.0;

/// Coarser lets a peak between two points read low, leaving the limiter to catch it.
const GRID_STEPS_PER_OCTAVE: f64 = 24.0;

/// A frequency as the response formula takes it, built once per point rather than per filter.
#[derive(Clone, Copy, Debug)]
struct Point {
    phi: f64,
    phi_squared: f64,
}

/// A biquad's squared magnitude, `|H|²`, over `φ = sin²(w/2)`, with the point its design centres
/// on, where a peaking filter's peak sits. Over `cos w`, a sub-bass filter's numerator and
/// denominator at a high rate are the few ulps left after terms of order one cancel.
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
        (n0 + n1 * point.phi + n2 * point.phi_squared)
            / (d0 + d1 * point.phi + d2 * point.phi_squared)
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
            numerator: power_over_phi([b0, b1, b2]),
            denominator: power_over_phi([1.0, a1, a2]),
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
    let half_sin = (PI * hz / sample_rate).sin();
    let phi = half_sin * half_sin;
    Point { phi, phi_squared: phi * phi }
}

/// `|t0 + t1·z⁻¹ + t2·z⁻²|²` on the unit circle, as the coefficients of 1, `φ` and `φ²`.
fn power_over_phi([t0, t1, t2]: [f64; 3]) -> [f64; 3] {
    let sum = t0 + t1 + t2;
    [sum * sum, -4.0 * (t0 * t1 + t1 * t2 + 4.0 * t0 * t2), 16.0 * t0 * t2]
}

#[cfg(test)]
#[path = "tests/response_tests.rs"]
mod tests;
