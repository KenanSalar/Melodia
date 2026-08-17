//! The colours an aurora backdrop washes over its base gradient, from the artwork's own seeds.
//!
//! **Each wash keeps the tone and chroma the record gave it**, clamped only where
//! [`crate::ui::backdrop::wash_cap`] binds — so a bright ochre stays brighter than a deep navy and
//! the album's value structure survives into the surface. The cap is a function of the geometry
//! below, which is why the coverage constants live here rather than in the paint.
//!
//! A fixed set, always — `Brush::interpolate` blends gradients only at a matching stop and element
//! count — and the quantizer can still answer short, a near-white sleeve coming back as one colour.
//! Hence [`tints`]'s filling rule.

use slint::{Color, Rgba8Pixel, SharedPixelBuffer};

use crate::services::material_you::{clamp_to_tone_band, hue_of, rotate_hue};
use crate::themes::color_with_alpha;
use crate::ui::backdrop::{SEED_COUNT, ThemeTokens, wash_cap};

/// Alpha each wash carries at its own corner, mirroring `aurora-backdrop.slint`'s four `peak`
/// bindings. Here because [`crate::ui::backdrop::wash_cap`] is a function of the coverage these
/// produce, so they stop being an implementation detail of the paint.
pub(crate) const BLOB_PEAKS: [f64; SEED_COUNT] = [0.5, 0.46, 0.42, 0.38];

/// How far a wash reaches, as a fraction of the host's diagonal — the same 1/√2 `aurora-backdrop`
/// spells to four places, and Amberol's own stop.
///
/// Over half the diagonal, so every ramp reaches the centre and the middle of the surface is not a
/// hole; under the whole of it, so the pair across a diagonal never meet and no pixel is all four
/// washes at strength.
pub(crate) const REACH_FRACTION: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// What the washes cover at the centre of the host: all four, each at the strength
/// [`REACH_FRACTION`] leaves it half a diagonal from its corner.
///
/// Alpha composites multiplicatively, so coverage is one minus what gets through all four. Spelled
/// as a loop because `Iterator::product` is not const, and it has to be for the assertions below.
pub(crate) const fn mid_coverage() -> f64 {
    let at_centre = 1.0 - 0.5 / REACH_FRACTION;
    let mut through = 1.0;
    let mut blob = 0;
    while blob < SEED_COUNT {
        through *= 1.0 - BLOB_PEAKS[blob] * at_centre;
        blob += 1;
    }
    1.0 - through
}

/// The most any pixel can be covered, over every host shape rather than any one of them.
///
/// The maximum is always at a corner, and the only wash that can reach it beside its own is the one
/// across the *short* edge — the far corners sit past the reach. As a host stretches that edge
/// shrinks against a diagonal that doesn't, so the neighbour's falloff goes to nothing and its
/// contribution to its own peak: the two strongest peaks bound every aspect there is. Real hosts sit
/// well under it — half on a square, two thirds on the bands — and over-stating it only costs the
/// surface brightness.
pub(crate) const fn peak_coverage() -> f64 {
    1.0 - (1.0 - BLOB_PEAKS[0]) * (1.0 - BLOB_PEAKS[1])
}

// The geometry's own bounds, as build failures rather than tests — `wash_cap` inverts through this
// coverage, so both ends are arithmetic rather than taste.
const _: () = assert!(peak_coverage() > 0.0, "the cap divides by the coverage");
const _: () = assert!(
    peak_coverage() < 1.0,
    "a wash covering a pixel outright leaves the theme's base showing nowhere"
);
const _: () = assert!(
    mid_coverage() < peak_coverage(),
    "the middle of the surface is covered harder than a corner, so the anchors have inverted"
);

/// Hue rotation for tint *n* when the quantizer had no seed for it, always applied to the first
/// colour that does exist — rotating from the previous fill would let the set walk away from the
/// album. A fan either side at the analogous step, so an invented set is harmonious by
/// construction; entry 0 is the identity, reaching it meaning no artwork at all.
const FILL_HUES: [f64; SEED_COUNT] = [0.0, 25.0, -25.0, 50.0];

/// How faintly a synthesized tint is laid on, against 1.0 for one the artwork offered. **A backdrop
/// may not invent variation the record doesn't have** — the fills of a one-hue sleeve differ only
/// in direction, and at full strength a stack of those is a lightness gradient it never had.
const FILL_WEIGHT: f32 = 0.3;

/// One wash's colour and how strongly it is laid on.
pub(crate) struct Tint {
    /// `0x00RRGGBB`, already clamped into the theme's wash band.
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

/// The washes, in paint order.
///
/// **A measured seed keeps its own hue.** An earlier pass pulled the set into an analogous arc,
/// reasoning that overlapping washes composite in sRGB and its midpoint between distant hues is
/// grey. That answers the wrong question: a cover of blue *and* red is a cover of two colours, and
/// clamping turned its red into a second violet. Separation is the Slint side's job, by giving each
/// wash a region of its own.
///
/// **It keeps its own tone and chroma too**, `clamp_to_tone_band` returning a colour inside the
/// band untouched — most album colours pass through it and only the extremes are moved. The band
/// is the theme's: bright enough that a dark variant's ink still clears its bar over the composite,
/// pastel enough that a light variant's does.
///
/// `theme`'s accent supplies the hue when the artwork gave nothing, and is never used to *pad* a
/// short list: one hue's worth of cover gets a full set of its own colours, not the app's.
pub(crate) fn tints(seeds: [Option<u32>; SEED_COUNT], theme: &ThemeTokens) -> [Tint; SEED_COUNT] {
    let origin = seeds.iter().flatten().next().copied().unwrap_or(theme.accent);
    let (min_tone, max_tone) = wash_cap(theme, peak_coverage());

    let ranked: [Tint; SEED_COUNT] = std::array::from_fn(|tint| {
        let (source, weight) = match seeds[tint] {
            Some(argb) => (argb, 1.0),
            None => (rotate_hue(origin, FILL_HUES[tint]), FILL_WEIGHT),
        };
        Tint {
            rgb: clamp_to_tone_band(source, min_tone, max_tone),
            weight,
        }
    });
    around_the_wheel(ranked)
}

/// Seat the washes so neighbours here are neighbours on the hue wheel, top-ranked first.
///
/// The blob positions are fixed, so the rank a colour arrives in decides which other colour it
/// overlaps — and `Score`'s ranking has nothing to do with hue. On a four-cover mosaic it put two
/// complementary pairs on top of each other, which composite toward grey, and the band painted flat
/// mauve out of four chroma-48 washes. Anchored at the dominant rather than sorted outright: it
/// carries the strongest wash and the base gradient under it is solved from the same seed.
fn around_the_wheel(ranked: [Tint; SEED_COUNT]) -> [Tint; SEED_COUNT] {
    let anchor = hue_of(ranked[0].rgb);
    let mut ordered = ranked;
    // One turn from the anchor, so the sort can't wrap past it and reopen the gap it closes.
    ordered[1..].sort_by(|a, b| {
        let turn = |tint: &Tint| (hue_of(tint.rgb) - anchor).rem_euclid(360.0);
        turn(a).total_cmp(&turn(b))
    });
    ordered
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
