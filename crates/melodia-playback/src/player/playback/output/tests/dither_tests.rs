//! Tests for the dither: which blocks it leaves alone, where it puts the rest, and that what it
//! leaves behind is noise rather than distortion.

use std::f32::consts::TAU;

use super::Dither;
use crate::player::playback::dsp::index_to_f32;
use crate::player::playback::output::encode::DeviceFormat;
use crate::player::playback::tests::helpers::bits;

/// A 16-bit grid's step, and a 24-bit one's, in a sample's own units.
const STEP_16: f32 = 1.0 / 32_768.0;
const STEP_24: f32 = 1.0 / 8_388_608.0;

/// Steps in full scale, for the formats the dither puts on a grid.
const SCALE_16: f64 = 32_768.0;
const SCALE_24: f64 = 8_388_608.0;

/// The most a dithered sample may move: the noise reaches one step either side, and the rounding
/// after it half a step more.
const MOST_STEPS_MOVED: f64 = 1.5;

/// How far the error left on a quiet sine may correlate with it. Rounding alone leaves it
/// correlating at several tenths, which is the distortion the dither exists to remove.
const MOST_CORRELATION: f64 = 0.02;

/// A block a bit-perfect path writes: every sample a whole number of steps, both ends of the range
/// and a negative zero among them.
fn on_grid(step: f32) -> Vec<f32> {
    vec![-1.0, -0.0, 0.0, step, -step, 1.0 - step, 0.25, -0.5 + step]
}

/// A block the chain changed: a 1 kHz sine at 48 kHz, at `amplitude`, which no grid holds.
fn changed(len: usize, amplitude: f32) -> Vec<f32> {
    (0..len).map(|i| amplitude * (TAU * index_to_f32(i) / 48.0).sin()).collect()
}

/// `block` after one dither, as a writer's first block meets it.
fn dithered(mut block: Vec<f32>, format: DeviceFormat) -> Vec<f32> {
    Dither::default().quantize(&mut block, format);
    block
}

/// A bit-perfect block is the one the dither must leave alone, or no claim stays bit-perfect.
#[test]
fn a_block_already_on_the_grid_passes_through_untouched() {
    let rows = [
        (DeviceFormat::S16, STEP_16),
        (DeviceFormat::S24Packed, STEP_24),
        (DeviceFormat::S24Low, STEP_24),
        (DeviceFormat::S24High, STEP_24),
    ];
    for (format, step) in rows {
        let block = on_grid(step);

        let out = dithered(block.clone(), format);

        assert_eq!(bits(&out), bits(&block), "{format}");
    }
}

/// What reaches `encode` or cpal is already an integer over the format's full scale, so the
/// rounding or truncation after it lands on the integer the dither chose.
#[test]
fn a_changed_block_is_put_on_the_grid() {
    let rows = [(DeviceFormat::S16, SCALE_16), (DeviceFormat::S24Low, SCALE_24)];
    for (format, scale) in rows {
        let out = dithered(changed(4_096, 0.3), format);

        let off_grid = out.iter().filter(|&&s| (f64::from(s) * scale).fract() != 0.0).count();

        assert_eq!(off_grid, 0, "{format}");
    }
}

#[test]
fn a_dithered_sample_moves_by_at_most_a_step_and_a_half() {
    let rows = [(DeviceFormat::S16, SCALE_16), (DeviceFormat::S24Low, SCALE_24)];
    for (format, scale) in rows {
        let block = changed(4_096, 0.3);

        let out = dithered(block.clone(), format);

        let most = block
            .iter()
            .zip(&out)
            .map(|(&before, &after)| (f64::from(after) - f64::from(before)).abs() * scale)
            .fold(0.0, f64::max);
        assert!(most <= MOST_STEPS_MOVED, "{format}: a sample moved {most} steps");
    }
}

/// An f32 carries 24 significant bits, so rounding one to 32 moves it by less than any converter's
/// own noise, and a float device takes the samples as they are.
#[test]
fn a_block_bound_for_32_bits_or_float_is_left_as_it_is() {
    for format in [DeviceFormat::S32, DeviceFormat::F32] {
        let block = changed(4_096, 0.3);

        let out = dithered(block.clone(), format);

        assert_eq!(bits(&out), bits(&block), "{format}");
    }
}

/// A sample the chain pushed to or past full scale is clipped to the format's own limits rather
/// than wrapped, whatever the noise drew. Minus full scale is left out on purpose: it sits on the
/// grid, and the noise may legitimately move it a step up.
#[test]
fn a_sample_at_or_past_full_scale_is_held_at_the_formats_limits() {
    let rows = [
        (DeviceFormat::S16, [1.0 - STEP_16, 1.0 - STEP_16, -1.0]),
        (DeviceFormat::S24Low, [1.0 - STEP_24, 1.0 - STEP_24, -1.0]),
    ];
    for (format, expected) in rows {
        let out = dithered(vec![1.0, 1.05, -1.05], format);

        assert_eq!(bits(&out), bits(&expected), "{format}");
    }
}

/// The reason the dither exists: rounding a quiet changed signal leaves an error that follows it,
/// which is heard as distortion. Dithered, the error is noise, unrelated to the music.
#[test]
fn the_error_left_on_a_quiet_changed_sine_is_uncorrelated_with_it() {
    let sine = changed(65_536, 2.5 * STEP_16);

    let out = dithered(sine.clone(), DeviceFormat::S16);

    let correlation = correlation(&sine, &out);
    assert!(
        correlation.abs() < MOST_CORRELATION,
        "the error correlates with the signal at {correlation:.3}"
    );
}

/// How closely the error `after` left on `before` follows `before`, from -1 to 1.
fn correlation(before: &[f32], after: &[f32]) -> f64 {
    let (mut cross, mut signal, mut error) = (0.0, 0.0, 0.0);
    for (&x, &y) in before.iter().zip(after) {
        let (x, e) = (f64::from(x), f64::from(y) - f64::from(x));
        cross += x * e;
        signal += x * x;
        error += e * e;
    }
    cross / (signal * error).sqrt()
}
