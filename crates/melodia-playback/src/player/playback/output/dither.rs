//! TPDF dither for a block narrowed to 16 or 24 bits after the chain changed it.
//!
//! Rounding a changed sample to the device's width leaves an error that follows the signal, which
//! at low levels is distortion rather than noise: the tail of a fade turns gritty instead of
//! sinking into hiss. Triangular noise one step either side, added before the rounding, makes the
//! error independent of the signal for a small rise in the noise floor.
//!
//! **A block the format holds exactly is left alone**, so a bit-perfect path stays one: a file's own
//! samples at unity sit on the grid already. The test is per block rather than per sample because
//! the samples of a changed signal that happen to land on the grid are not a separate signal, and
//! leaving just those undithered ties the error back to the music.

use melodia_audio::player::source::audio::Sample;

use super::encode::{DeviceFormat, full_scale, nearest_step};

/// The widest grid dithered. An `f32` carries 24 significant bits, so rounding one to 32 moves it
/// by at most 2⁻³², under any converter's own noise.
const WIDEST_DITHERED_BITS: u8 = 24;

/// Knuth's MMIX constants, which give the full 2⁶⁴ period.
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
const INCREMENT: u64 = 1_442_695_040_888_963_407;
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// What one uniform draw spans, so the difference of two lands inside one step either side.
const DRAW_SPAN: f64 = 65_536.0;

/// One stream's dither: the generator its noise is drawn from.
pub struct Dither {
    state: u64,
}

impl Default for Dither {
    fn default() -> Self {
        Self { state: SEED }
    }
}

impl Dither {
    /// Put `block` on `format`'s grid where the chain moved it off, or past full scale: each sample
    /// rounded after the noise, then held inside the range. A block the format already holds
    /// exactly, or a format too wide to need it, is left as it is.
    ///
    /// Every result is an integer over a power of two, which an `f32` holds exactly, so whatever
    /// rounds or truncates it next lands on the integer meant.
    pub fn quantize(&mut self, block: &mut [Sample], format: DeviceFormat) {
        let Some(bits) = format.integer_bits().filter(|&bits| bits <= WIDEST_DITHERED_BITS) else {
            return;
        };
        let scale = full_scale(bits);
        if block.iter().all(|&sample| holds_exactly(sample, scale)) {
            return;
        }
        for sample in block {
            let level = f64::from(*sample) * scale + self.triangular();
            *sample = narrow(nearest_step(level, scale) / scale);
        }
    }

    /// Noise spanning one step either side with a triangular density: the difference of two
    /// uniform draws. Both come from the state's high half, since an LCG's low bits repeat on
    /// short periods.
    #[expect(clippy::cast_possible_truncation, reason = "each draw keeps sixteen of the high bits")]
    fn triangular(&mut self) -> f64 {
        self.state = self.state.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT);
        let first = (self.state >> 48) as u16;
        let second = (self.state >> 32) as u16;
        (f64::from(first) - f64::from(second)) / DRAW_SPAN
    }
}

/// Whether the format holds `sample` as it is: on a step of the grid, and short of full scale.
fn holds_exactly(sample: Sample, scale: f64) -> bool {
    let level = f64::from(sample) * scale;
    level.fract() == 0.0 && (-scale..scale).contains(&level)
}

#[expect(clippy::cast_possible_truncation, reason = "a 24-bit value over a power of two is exact")]
fn narrow(sample: f64) -> Sample {
    sample as Sample
}
