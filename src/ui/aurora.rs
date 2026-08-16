//! The colours an aurora backdrop washes over its base gradient, from the artwork's own seeds.
//!
//! [`crate::ui::backdrop`]'s counterpart: that module solves every *foreground* tone for contrast,
//! this one owns the surface underneath and states its brightest point, which is the only thing a
//! WCAG ratio can be targeted against.
//!
//! A fixed set, always: `Score` returns fewer than asked and never pads, and `Brush::interpolate`
//! blends gradients only at a matching stop and element count.

use slint::{Color, Rgba8Pixel, SharedPixelBuffer};

use crate::services::material_you::{hue_of, rotate_hue, to_tone_with_chroma};
use crate::themes::color_with_alpha;
use crate::ui::backdrop::SEED_COUNT;

/// Tone every tint is driven to, so hue is the only axis they differ on — a brighter wash reads as
/// light falling on the surface rather than as the surface's own colour.
///
/// **Above [`crate::ui::backdrop::TARGET_BACKDROP_TONE`] on purpose**: that ceiling belongs to the
/// composite, not to a single layer under it, and no wash is opaque. Held down to it instead, this
/// went grey — chroma is bounded by tone.
pub(crate) const TINT_TONE: f64 = 36.0;

/// The brightest tone the finished stack presents — what [`crate::ui::backdrop`] solves its
/// foreground tiers against on this surface.
///
/// Stated rather than measured, there being no buffer. [`TINT_TONE`] bounds it absolutely, and the
/// washes are spread far enough apart that coverage stays well under 1: over a base at
/// `backdrop::FLOOR_TONE_START` the peak lands at tone 29.8 on a square host, the worst aspect
/// because the spread is tightest there. Deliberately not a mean over the tint colours, which
/// understates bright regions for [`crate::ui::backdrop::luma_p90`]'s reason.
pub(crate) const PEAK_TONE: f64 = 31.0;

// Build failures rather than tests: a peak above either bound has no symptom on screen, every tier
// saturating at its band floor across the whole legal range today.
const _: () = assert!(
    PEAK_TONE <= TINT_TONE,
    "a stack of washes cannot be brighter than the tone every wash is driven to"
);
const _: () = assert!(
    PEAK_TONE <= crate::ui::backdrop::TARGET_BACKDROP_TONE,
    "the peak sits above the band the foreground tiers are solved for"
);

/// Chroma floor, and the reason the surface carries the record's colour rather than a wash of it.
///
/// `Score` ranks by how *usable* a colour is, not how saturated, so a cover's second and third
/// seeds are routinely a near-white and a near-black. Taken as they came the dull two dilute the
/// good one and the surface converges on grey — which more alpha makes worse, not better. Reached
/// only by artwork that is itself colourful; see [`chroma_band`].
const TINT_MIN_CHROMA: f64 = 36.0;

/// Ceiling against a pathological seed; the floor above does the shaping. sRGB stops well short of
/// it at this tone for most hues anyway.
const TINT_MAX_CHROMA: f64 = 48.0;

/// Artwork chroma at which the band above applies in full. Ordinary colourful sleeves measure
/// 22–24 and a black-and-white one 5, so this sits in that gap with room either side.
const TINT_CHROMA_REFERENCE: f64 = 20.0;

/// The chroma band a tint is held to, scaled by how colourful `artwork_chroma` says the cover is.
///
/// **The backdrop may not be more of a colour than the record is.** A black-and-white sleeve still
/// quantizes to seeds carrying a few points of chroma, and the seeds can't be asked which case they
/// are — a greyscale cover's dimmest sits *below* a colourful one's, so only the whole image
/// separates them. **Squared**, because a proportional share of a tint covering the whole surface
/// still reads as a colour; still a curve, so near-identical covers can't land either side of a
/// cliff.
fn chroma_band(artwork_chroma: f64) -> (f64, f64) {
    let colourfulness = (artwork_chroma / TINT_CHROMA_REFERENCE).clamp(0.0, 1.0);
    let scale = colourfulness * colourfulness;
    (TINT_MIN_CHROMA * scale, TINT_MAX_CHROMA * scale)
}

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
    /// `0x00RRGGBB`, already driven into the tint band.
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
/// `artwork_chroma` decides how far the floor may lift a dull seed — a greyscale sleeve keeps its
/// greys. `fallback` supplies the hue when the artwork gave nothing, and is never used to *pad* a
/// short list: one hue's worth of cover gets a full set of its own colours, not the app's.
pub(crate) fn tints(
    seeds: [Option<u32>; SEED_COUNT],
    artwork_chroma: f64,
    fallback: u32,
) -> [Tint; SEED_COUNT] {
    let origin = seeds.iter().flatten().next().copied().unwrap_or(fallback);
    let (floor, ceiling) = chroma_band(artwork_chroma);

    let ranked: [Tint; SEED_COUNT] = std::array::from_fn(|tint| {
        let (source, weight) = match seeds[tint] {
            Some(argb) => (argb, 1.0),
            None => (rotate_hue(origin, FILL_HUES[tint]), FILL_WEIGHT),
        };
        Tint {
            rgb: to_tone_with_chroma(source, TINT_TONE, floor, ceiling),
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
