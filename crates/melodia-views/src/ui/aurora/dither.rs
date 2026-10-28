//! The grain laid over an aurora backdrop. `FemtoVG` has no dithering pass, so washes this wide
//! and shallow band into stripes; the blur escapes only through a photograph's own grain.

use slint::{Rgba8Pixel, SharedPixelBuffer};

/// Big enough that the repeat carries no structure, the generator wrapping so the tile is
/// seamless. `u32`, the buffer's own unit.
const DITHER_TILE_SIDE: u32 = 64;

const DITHER_TILE_PIXELS: usize = (DITHER_TILE_SIDE * DITHER_TILE_SIDE) as usize;

/// One 8-bit level: the tile spans the full byte range, so this moves what is under it by exactly
/// one quantization step, enough to break a contour and not enough to see.
const DITHER_ALPHA: u8 = 1;

/// High-pass iterations shaping the noise toward blue; neighbour contrast stops improving past
/// eight.
const BLUE_NOISE_PASSES: usize = 8;

const _: () = assert!(
    BLUE_NOISE_PASSES > 0,
    "with no pass, the levels below are raw white noise rather than ranks, and the scale is wrong"
);

/// A tile of neutral blue noise breaking up 8-bit banding. Blue rather than white, whose
/// low-frequency energy reads as blotches.
pub fn dither_tile() -> SharedPixelBuffer<Rgba8Pixel> {
    let levels = blue_noise_levels();
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(DITHER_TILE_SIDE, DITHER_TILE_SIDE);
    for (pixel, level) in buf.make_mut_slice().iter_mut().zip(levels) {
        *pixel = Rgba8Pixel { r: level, g: level, b: level, a: DITHER_ALPHA };
    }
    buf
}

/// White noise high-passed and re-ranked to uniform, repeatedly: the cheap way to a blue spectrum.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the tile is 64×64, so every rank is exactly representable and lands in 0..=255"
)]
fn blue_noise_levels() -> Vec<u8> {
    // Constant seed, so a rendering difference can be reproduced.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut values: Vec<f32> = (0..DITHER_TILE_PIXELS)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let sample = state.to_le_bytes();
            f32::from(u16::from_le_bytes([sample[0], sample[1]]))
        })
        .collect();

    for _ in 0..BLUE_NOISE_PASSES {
        let low = low_pass_toroidal(&values);
        let high: Vec<f32> = values.iter().zip(&low).map(|(value, low)| value - low).collect();
        values = rank_of_each(&high);
    }

    // Already a rank permutation, the last pass ending in `rank_of_each`. Scaled by the count, not
    // the last index, which would leave the histogram a bin short of flat and so dither parts of
    // the surface differently.
    values.iter().map(|rank| (*rank * 256.0 / DITHER_TILE_PIXELS as f32) as u8).collect()
}

/// Each value's rank; re-ranking keeps the distribution uniform across passes.
fn rank_of_each(values: &[f32]) -> Vec<f32> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_unstable_by(|a, b| values[*a].total_cmp(&values[*b]));

    let mut ranks = vec![0.0_f32; values.len()];
    for (rank, index) in order.into_iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "4096 ranks are exact in f32")]
        {
            ranks[index] = rank as f32;
        }
    }
    ranks
}

/// Separable binomial blur that wraps at the edges, so the tile stays seamless rather than showing
/// a grid at its own pitch.
fn low_pass_toroidal(source: &[f32]) -> Vec<f32> {
    // Narrow on purpose: a wide blur's low cutoff leaves most of the spectrum untouched.
    const KERNEL: [f32; 3] = [1.0, 2.0, 1.0];
    const KERNEL_SUM: f32 = 4.0;
    const RADIUS: usize = 1;

    let side = DITHER_TILE_SIDE as usize;
    let mut rows = vec![0.0_f32; source.len()];
    for row in 0..side {
        for col in 0..side {
            let mut sum = 0.0;
            for (tap, weight) in KERNEL.iter().enumerate() {
                sum += weight * source[row * side + (col + tap + side - RADIUS) % side];
            }
            rows[row * side + col] = sum / KERNEL_SUM;
        }
    }

    let mut blurred = vec![0.0_f32; source.len()];
    for row in 0..side {
        for col in 0..side {
            let mut sum = 0.0;
            for (tap, weight) in KERNEL.iter().enumerate() {
                sum += weight * rows[((row + tap + side - RADIUS) % side) * side + col];
            }
            blurred[row * side + col] = sum / KERNEL_SUM;
        }
    }
    blurred
}

#[cfg(test)]
#[path = "tests/dither_tests.rs"]
mod tests;
