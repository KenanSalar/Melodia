//! The colours an aurora backdrop washes over its base gradient, from the artwork's own seeds.
//!
//! [`crate::ui::backdrop`]'s counterpart: that module solves every *foreground* tone for contrast,
//! this one owns the surface underneath. The split is what the WCAG solve needs — a ratio can only
//! be targeted against a backdrop whose brightest point is known, and stating that point is the
//! whole job here.
//!
//! A fixed set, always. `Score` returns fewer when the artwork can't separate that many hues and
//! never pads, so the filling rule below is load-bearing rather than defensive — and the count
//! reaching Slint has to be fixed either way, `Brush::interpolate` blending gradients only at a
//! matching stop and element count.

use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::services::material_you::{rotate_hue, to_tone_with_chroma};
use crate::ui::backdrop::SEED_COUNT;

/// Tone every tint is driven to — one for all of them, so hue is the only axis they differ on. A
/// tint brighter than its neighbours turns its wash into a lightness ramp, which reads as light
/// falling on the surface rather than as the surface's own colour.
///
/// **Above [`crate::ui::backdrop::TARGET_BACKDROP_TONE`] on purpose.** That ceiling belongs to the
/// composite the foreground is solved against, not to any single layer under it; no wash is ever
/// opaque, so 36 over a floor at 18 peaks around 31. Holding each tint down to the ceiling instead
/// spends the whole margin for nothing and is what made this grey — chroma is bounded by tone.
pub(crate) const TINT_TONE: f64 = 36.0;

/// The brightest tone the finished stack presents — what [`crate::ui::backdrop`] solves its
/// foreground tiers against when this is the surface they sit on.
///
/// **Stated rather than measured, there being no buffer to measure.** Two bounds fix it. Every
/// wash is driven to [`TINT_TONE`], and compositing a colour over something darker lands between
/// the two, so 36 is a ceiling no stacking can pass. And the geometry keeps coverage well under
/// 1: the blob rects are 1.3 diagonals square, so their ramps die 0.643 diagonals out while their
/// centres sit 0.35 out and 0.495 apart, leaving the strongest point its own 0.5 plus about a
/// tenth from each neighbour. Six tenths of tone 36 over a base starting at
/// `backdrop::FLOOR_TONE_START` lands near 29; this carries the headroom, and stays under
/// [`crate::ui::backdrop::TARGET_BACKDROP_TONE`] so the tiers keep the bands they already solve in.
///
/// **Not a mean over the tint colours.** Understating bright regions is the exact failure the
/// blur's percentile exists to avoid, and a blob centre is the smeared wordmark that argument was
/// about.
pub(crate) const PEAK_TONE: f64 = 31.0;

// Both of [`PEAK_TONE`]'s bounds, as build failures rather than prose: a peak above either has no
// symptom on screen, every tier saturating at its band floor across the whole legal range today.
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
/// seeds are routinely a near-white and a near-black — measured, a dominant at chroma 31 beside 12
/// and 15. Taken as they came the dull two dilute the good one and the surface converges on grey,
/// the harder they are laid on the greyer it gets, which is why reaching for more alpha makes this
/// worse rather than better.
///
/// Both reached only by artwork that is itself colourful — see [`chroma_band`].
const TINT_MIN_CHROMA: f64 = 36.0;

/// Ceiling against a pathological seed; the floor above does the shaping. sRGB stops well short of
/// it at this tone for most hues anyway.
const TINT_MAX_CHROMA: f64 = 48.0;

/// Artwork chroma at which the band above applies in full.
///
/// Measured, ordinary colourful sleeves sit at 22–24 and a black-and-white one at 5, so this is
/// inside that gap with room either side — and near where `Score`'s own per-cluster cutoff puts
/// the boundary between a hue and a rounding error.
const TINT_CHROMA_REFERENCE: f64 = 20.0;

/// The chroma band a tint is held to, scaled by how colourful `artwork_chroma` says the cover is.
///
/// **The backdrop may not be more of a colour than the record is.** A black-and-white sleeve still
/// quantizes to seeds carrying a few points of chroma — noise and a hint of tint in a near-black
/// field — and neither bound may take them at face value: lifted to the floor they paint it red
/// and violet, and left at their own 9 they still wash the whole surface mauve, because a tint
/// covering everything needs very little chroma to read as a colour. Nor can the seeds be asked
/// which case they are: measured, a greyscale cover's 9.4 sits *below* a colourful one's 12.6, and
/// only the whole image separates them.
///
/// **Squared**, so colour falls away faster than the artwork does. Proportional scaling leaves a
/// near-grey cover a proportional share of its tint, which is exactly the mauve being removed;
/// squaring takes a cover at a quarter of the reference down to a sixteenth of the band. Still a
/// curve rather than a threshold, so two near-identical covers can't land either side of a cliff.
fn chroma_band(artwork_chroma: f64) -> (f64, f64) {
    let colourfulness = (artwork_chroma / TINT_CHROMA_REFERENCE).clamp(0.0, 1.0);
    let scale = colourfulness * colourfulness;
    (TINT_MIN_CHROMA * scale, TINT_MAX_CHROMA * scale)
}

/// Hue rotation for tint *n* when the quantizer had no seed for it, applied to the first colour
/// that does exist — rotating from the previous tint instead would let the fills walk away from
/// the album. A fan either side of the source at 25°, the analogous step, so an invented set is
/// harmonious by construction; the fourth continues it rather than opening a second gap. Tint 0's
/// entry is the identity: reaching it means no artwork at all, and the fallback hue is the answer.
const FILL_HUES: [f64; SEED_COUNT] = [0.0, 25.0, -25.0, 50.0];

/// How faintly a synthesized tint is laid on, against 1.0 for one the artwork offered.
///
/// **A backdrop may not invent variation the record doesn't have.** On a sleeve that quantized to
/// one hue the rotated fills differ only in *direction*, and at full strength a stack of those is
/// a lightness gradient the record never had. Weighted down, a monochrome sleeve settles onto its
/// own base — which is what its blur looked like — while a many-hued cover still gets the full set.
const FILL_WEIGHT: f32 = 0.3;

/// One wash's colour and how strongly it is laid on.
pub(crate) struct Tint {
    /// `0x00RRGGBB`, already driven into the tint band.
    pub rgb: u32,
    /// Multiplies the falloff the Slint side paints, rather than replacing it.
    pub weight: f32,
}

/// The washes, in paint order.
///
/// **A measured seed keeps its own hue.** An earlier pass pulled the set into an analogous arc,
/// on the reasoning that overlapping washes composite in sRGB and its midpoint between distant
/// hues is grey — but that answers the wrong question. A cover of blue *and* red is a cover of two
/// colours, and clamping turned its red into a second violet: measured, three seeds 231°/17°/304°
/// came out 231°/271°/270°, and the record's most vivid colour was the one thrown away. What keeps
/// the overlaps from going grey is that the blobs are spread far enough apart to have regions of
/// their own, which is the Slint side's business.
///
/// `artwork_chroma` is how colourful the cover is overall, and decides how far the floor is
/// allowed to lift a dull seed — a greyscale sleeve keeps its greys.
///
/// `fallback` supplies the hue when the artwork gave nothing — the theme accent, as everywhere
/// else on this surface. It is never used to *pad* a short list: a cover that quantized to one
/// hue gets a full set of its own colours, not a short one padded with the app's.
pub(crate) fn tints(
    seeds: [Option<u32>; SEED_COUNT],
    artwork_chroma: f64,
    fallback: u32,
) -> [Tint; SEED_COUNT] {
    let origin = seeds.iter().flatten().next().copied().unwrap_or(fallback);
    let (floor, ceiling) = chroma_band(artwork_chroma);

    std::array::from_fn(|tint| {
        let (source, weight) = match seeds[tint] {
            Some(argb) => (argb, 1.0),
            None => (rotate_hue(origin, FILL_HUES[tint]), FILL_WEIGHT),
        };
        Tint {
            rgb: to_tone_with_chroma(source, TINT_TONE, floor, ceiling),
            weight,
        }
    })
}

/// Side of the noise tile. Big enough that the repeat carries no structure to lock onto — the
/// generator wraps, so the tile is seamless — and small enough not to be worth measuring.
const DITHER_TILE_SIDE: usize = 64;

const DITHER_TILE_PIXELS: usize = DITHER_TILE_SIDE * DITHER_TILE_SIDE;

/// Alpha the tile is composited at. **One, because one 8-bit level is the whole prescription** —
/// the tile spans the full byte range, so `1/255` moves what is under it by exactly one
/// quantization step, enough to break a contour and not enough to see. More is not more dithering
/// but visible noise: at six this drew as a film of dust.
const DITHER_ALPHA: u8 = 1;

/// High-pass iterations shaping the noise toward blue. Measured on the tile's own neighbour
/// contrast — white noise sits at 0.33, this reaches 0.39 by the eighth pass and then flattens,
/// so anything past it is sorting for nothing.
const BLUE_NOISE_PASSES: usize = 8;

/// A tile of neutral blue noise, laid over the backdrop to break up 8-bit banding.
///
/// **`FemtoVG` has no dithering pass**, so a ramp this wide and this shallow quantizes into
/// visible bands — the aurora's base runs about two dozen sRGB levels across the diagonal, a flat
/// stripe every eighty pixels or so. The blur this replaces escaped that only because a photograph
/// carries its own grain, and grain is dither.
///
/// **Blue rather than white** is the difference between invisible and grubby: white noise keeps
/// energy in the low frequencies the eye is most sensitive to and reads as blotches. That matters
/// more than usual at one level of amplitude, where the tile is nearly a one-bit pattern and how
/// it spaces itself is all there is to see.
///
/// Generated once, uniform by construction and mid-grey on average — the *variance* is the point.
/// One tile serves every backdrop at every size, which is what separates it from the per-cover
/// buffers this feature deletes.
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
