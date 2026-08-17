//! The colours an aurora backdrop washes over its base, from the artwork's own seeds.
//!
//! **Each wash is the quantizer's answer, untouched.** No tone band, no hue seating, no reordering:
//! a sleeve's value structure reaches the backdrop intact, so one region is genuinely light and
//! another genuinely dark. Bounding the set to keep the theme's ink legible everywhere is what
//! flattens it — every wash above the bound lands on the bound and a four-colour record paints as
//! two — so contrast against `Theme.text` is whatever the cover gives, and the neutral chrome tier
//! ([`crate::ui::backdrop::theme_backdrop`]) carries the readable half.
//!
//! A fixed set, always — `Brush::interpolate` blends gradients only at a matching stop and element
//! count — and the quantizer can still answer short, a near-white sleeve coming back as one colour.
//! Hence [`tints`]'s filling rule, which an entry with no cover at all reaches through the same
//! door: the accent stands in as its one seed.

use slint::{Color, Rgba8Pixel, SharedPixelBuffer};

use crate::services::material_you::rotate_hue;
use crate::themes::color_with_alpha;
use crate::ui::backdrop::{SEED_COUNT, ThemeTokens};

/// Washes the paint lays down, against [`SEED_COUNT`] the quantizer is asked for.
///
/// **Three, and the two numbers are deliberately not one.** `aurora-backdrop.slint` mounts three
/// sweeps; median cut is still asked for four boxes because it splits to a target, so cutting a
/// palette to three directly yields different boxes rather than the same ones minus the last.
pub(crate) const WASH_COUNT: usize = 3;

const _: () = assert!(
    WASH_COUNT <= SEED_COUNT,
    "the paint wants more washes than the quantizer is asked for, so one would be a default"
);

/// Hue rotation for wash *n* when the quantizer had no seed for it, always applied to the first
/// colour that does exist — rotating from the previous fill would let the set walk away from the
/// album. A fan either side at the analogous step, so an invented set is harmonious by
/// construction. Entry 0 is the identity and unreachable, [`tints`] never leaving the first slot
/// empty.
const FILL_HUES: [f64; WASH_COUNT] = [0.0, 25.0, -25.0];

/// How faintly a synthesized tint is laid on, against 1.0 for one the artwork offered. **A backdrop
/// may not invent variation the record doesn't have** — the fills of a one-hue sleeve differ only
/// in direction, and at full strength a stack of those is a lightness gradient it never had.
const FILL_WEIGHT: f32 = 0.3;

/// One wash's colour and how strongly it is laid on.
pub(crate) struct Tint {
    /// `0x00RRGGBB`, exactly as the quantizer answered.
    pub rgb: u32,
    /// Multiplies the falloff the Slint side paints, rather than replacing it.
    pub weight: f32,
}

impl Tint {
    /// The wash as both backdrop tiers take it, weight riding in the alpha channel so that how
    /// strongly it is laid on stays independent of the shape it is laid on with.
    pub(crate) fn to_color(&self) -> Color {
        color_with_alpha(self.rgb, self.weight)
    }
}

/// The washes, in the order the quantizer ranked them.
///
/// **Nothing is done to them** — median cut's own answer, in median cut's own order, at the alpha
/// the paint gives it. The tone band and the hue seating that used to sit here each made the
/// surface safe for the theme's ink at the cost of the thing the backdrop is for: the band merged
/// every wash above it onto one tone, and the seating spent the ranking on hue adjacency the
/// sweeps' geometry no longer needs, each owning an edge rather than a corner.
///
/// `theme`'s accent never *pads* a short list: one hue's worth of cover gets a full set of its own
/// colours, not the app's. It stands in only for an *empty* one — an entry with no artwork — and it
/// stands in as a **seed** rather than as a rotation origin: three fills with nothing at full weight
/// behind them wash at [`FILL_WEIGHT`] throughout and read as an unpainted surface, where one seed
/// and two fills is the set a sleeve that quantized to a single colour already gets.
pub(crate) fn tints(seeds: [Option<u32>; SEED_COUNT], theme: &ThemeTokens) -> [Tint; WASH_COUNT] {
    let origin = seeds.iter().flatten().next().copied().unwrap_or(theme.accent);
    let mut seeds = seeds;
    if seeds.iter().all(Option::is_none) {
        seeds[0] = Some(origin);
    }

    std::array::from_fn(|wash| match seeds[wash] {
        Some(argb) => Tint {
            rgb: argb,
            weight: 1.0,
        },
        None => Tint {
            rgb: rotate_hue(origin, FILL_HUES[wash]),
            weight: FILL_WEIGHT,
        },
    })
}

/// Side of the noise tile. Big enough that the repeat carries no structure to lock onto — the
/// generator wraps, so the tile is seamless — and small enough not to be worth measuring.
const DITHER_TILE_SIDE: usize = 64;

const DITHER_TILE_PIXELS: usize = DITHER_TILE_SIDE * DITHER_TILE_SIDE;

/// Alpha the tile is composited at. **One 8-bit level is the whole prescription**: the tile spans
/// the full byte range, so `1/255` moves what is under it by exactly one quantization step — enough
/// to break a contour, not enough to see. At six this drew as a film of dust.
const DITHER_ALPHA: u8 = 1;

/// High-pass iterations shaping the noise toward blue. Neighbour contrast reaches 0.39 by the
/// eighth pass against white noise's 0.33 and then flattens, so anything past it sorts for nothing.
const BLUE_NOISE_PASSES: usize = 8;

/// A tile of neutral blue noise, laid over the backdrop to break up 8-bit banding.
///
/// **`FemtoVG` has no dithering pass**, so a ramp this wide and this shallow quantizes into visible
/// stripes; the blur beside it escapes that only because a photograph carries its own grain.
/// **Blue rather than white** is the difference between invisible and grubby — white noise keeps
/// energy in the low frequencies the eye is most sensitive to and reads as blotches.
pub fn dither_tile() -> SharedPixelBuffer<Rgba8Pixel> {
    let levels = blue_noise_levels();
    let side = u32::try_from(DITHER_TILE_SIDE).unwrap_or(u32::MAX);

    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(side, side);
    for (pixel, level) in buf.make_mut_slice().iter_mut().zip(levels) {
        *pixel = Rgba8Pixel {
            r: level,
            g: level,
            b: level,
            a: DITHER_ALPHA,
        };
    }
    buf
}

/// White noise, repeatedly high-passed and re-flattened, which is the cheap way to a blue
/// spectrum: subtracting a blurred copy removes the low frequencies, and re-ranking puts the
/// histogram back to uniform so the next pass has the same amount of signal to work on.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the tile is 64×64, so every rank is exactly representable and lands in 0..=255"
)]
fn blue_noise_levels() -> Vec<u8> {
    // Constant seed: a tile that changed between runs would be a rendering difference nobody
    // could reproduce.
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

    // Scaled by the *count*, not the last index: dividing by 255/(n-1) puts the single top rank
    // alone in level 255 and leaves the histogram one bin short of flat, which is the one shape
    // this must not have — whether a pixel rounds up is decided against a fixed threshold, so an
    // uneven histogram dithers parts of the surface differently.
    let ranks = rank_of_each(&values);
    ranks.iter().map(|rank| (*rank * 256.0 / DITHER_TILE_PIXELS as f32) as u8).collect()
}

/// Each value's position in sorted order. Re-ranking is what keeps the distribution uniform
/// across passes — a plain high-pass leaves it bunched around zero, and a bunched histogram
/// dithers unevenly.
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

/// Separable binomial blur that **wraps at the edges**, which is what keeps the tile seamless:
/// clamped edges leave the border rows correlated differently from the interior, and the seam then
/// shows as a grid at the tile's own pitch.
fn low_pass_toroidal(source: &[f32]) -> Vec<f32> {
    // Narrow on purpose — a wide blur has a low cutoff and leaves most of the spectrum untouched.
    // Measured, the three-tap beats a seven-tap at every pass count.
    const KERNEL: [f32; 3] = [1.0, 2.0, 1.0];
    const KERNEL_SUM: f32 = 4.0;
    const RADIUS: usize = 1;

    let side = DITHER_TILE_SIDE;
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
#[path = "tests/aurora_tests.rs"]
mod tests;
