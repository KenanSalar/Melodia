//! The safety limiter: transparent until near full scale, then it pins peaks at the threshold. In
//! `EqSource`'s active path only, so audio with nothing engaged is never touched.

use super::dsp::{db_to_linear, linear_to_db};

const LIMITER_THRESHOLD_DB: f32 = -1.0;
const LIMITER_KNEE_DB: f32 = 4.0;
const LIMITER_ATTACK_S: f32 = 0.005;
const LIMITER_RELEASE_S: f32 = 0.100;

/// Soft-knee feed-forward peak limiter (Giannoulis et al. 2012): one gain per frame from its peak,
/// smoothed with separate attack and release, and coupled across channels to keep the image.
pub(crate) struct Limiter {
    /// Smoothed linear gain (≤ 1.0); starts at unity.
    gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    /// Linear, so [`Self::target_gain`] answers a quiet frame without a `log10`.
    knee_low_linear: f32,
    /// Linear, the curve having a closed linear form past it.
    knee_high_linear: f32,
    /// Above the knee, the gain's numerator.
    threshold_linear: f32,
}

impl Limiter {
    pub(crate) fn new(frame_rate: f32) -> Self {
        Self {
            gain: 1.0,
            attack_coeff: smoothing_coeff(LIMITER_ATTACK_S, frame_rate),
            release_coeff: smoothing_coeff(LIMITER_RELEASE_S, frame_rate),
            knee_low_linear: db_to_linear(LIMITER_THRESHOLD_DB - LIMITER_KNEE_DB / 2.0),
            knee_high_linear: db_to_linear(LIMITER_THRESHOLD_DB + LIMITER_KNEE_DB / 2.0),
            threshold_linear: db_to_linear(LIMITER_THRESHOLD_DB),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.gain = 1.0;
    }

    pub(crate) fn process(&mut self, peak: f32) -> f32 {
        let target = self.target_gain(peak);
        let coeff = if target < self.gain { self.attack_coeff } else { self.release_coeff };
        self.gain = coeff.mul_add(self.gain, (1.0 - coeff) * target);
        self.gain
    }

    /// The soft-knee curve with an unbounded ratio above the knee.
    fn target_gain(&self, peak: f32) -> f32 {
        // Unity below the knee, so the common quiet frame skips the `log10`.
        if peak <= self.knee_low_linear {
            return 1.0;
        }
        // The output pins at the threshold; `10^(-over/20)` in dB expands to exactly this.
        if peak >= self.knee_high_linear {
            return self.threshold_linear / peak;
        }
        // The quadratic knee, meeting both guards' answers at its edges.
        let over = linear_to_db(peak) - LIMITER_THRESHOLD_DB;
        let k = over + LIMITER_KNEE_DB / 2.0;
        db_to_linear(-(k * k) / (2.0 * LIMITER_KNEE_DB))
    }
}

/// One-pole smoothing coefficient for a time constant at a given update rate:
/// the smoothed value moves `(1 - coeff)` of the way to its target each update.
fn smoothing_coeff(time_s: f32, rate: f32) -> f32 {
    if rate > 0.0 && time_s > 0.0 { (-1.0 / (time_s * rate)).exp() } else { 0.0 }
}
