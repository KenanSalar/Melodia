//! How bright an artwork-derived backdrop is, and which colours survive on it.
//!
//! **[`BackdropKind`] is where the two backdrops part, and they share nothing past it.** The blur
//! is a photograph and can be any brightness; the aurora is a stack of our own washes over the
//! theme's own base.
//!
//! **The blur is measured.** [`luma_p90`] reads the cover, [`scrim_alpha`] solves an opacity
//! driving the *composited* result into a known dark band ([`composited_tone`]), and each
//! foreground tier's HCT tone is solved against that for a WCAG target. That order is
//! load-bearing: adapting the foreground first answers a bright cover with a *polarity flip* past
//! the black/white crossover rather than a darker accent, and a blurred cover isn't uniform
//! besides, so no global foreground decision serves a backdrop bright in one corner and dark in
//! another. Pin the backdrop and every cover ends up dark, so one light foreground is correct
//! everywhere.
//!
//! **The aurora neither measures nor bounds.** Its washes are the quantizer's answer untouched and
//! the tiers over them are the theme's own tokens, the chrome a neutral ink the washes read
//! through. It can afford that where the blur can't: three ramps of album colour at partial alpha
//! over the theme's own base stay near that base, where a blurred photograph fills the surface and
//! can be any brightness at all. [`crate::ui::aurora`] argues what the missing guarantee buys.
//!
//! **The blur's measurement comes off the decoded cover, never the rendered frame** — the chrome
//! tints itself off the same accent feeding the backdrop, so sampling the composite closes a
//! feedback loop.

use std::sync::LazyLock;

use material_colors::color::{linearized, lstar_from_y, y_from_lstar};
use material_colors::contrast;
use slint::{Brush, ComponentHandle};

use crate::services::material_you::{clamp_to_tone_band, population_seeds, to_tone_capped_chroma};
use crate::themes::{brush, brush_to_rgb, brush_with_alpha};
use crate::ui::artwork_cache::BlurSpec;
use crate::{AppWindow, Theme as ThemeGlobal};

/// HCT tone the composited blur is driven down to. Below it a light hue-carrying chrome tone
/// clears WCAG's 3:1 non-text bar with margin and body text clears 4.5:1 without washing out.
/// Raising it shows more artwork and costs contrast headroom.
pub(crate) const TARGET_BACKDROP_TONE: f64 = 32.0;

/// Floor on the solved scrim opacity, deliberately light: below the target tone there is nothing
/// left to darken, so a black sleeve shows more of itself.
const SCRIM_ALPHA_MIN: f32 = 0.30;

/// Ceiling, keeping a pathological cover from flattening into a plain dark rectangle.
const SCRIM_ALPHA_MAX: f32 = 0.82;

/// Two covers from the same album differ by a fraction of a percent; snapping keeps them on one
/// value rather than shimmering between neighbours as the user skips.
const SCRIM_ALPHA_STEP: f32 = 0.01;

/// Scrim fill tone — album-tinted near-black. sRGB holds almost no chroma this low, so the tint is
/// a hint and the opacity solve is unaffected by which hue it is.
const SCRIM_TONE: f64 = 8.0;

/// Gradient-floor stops, in HCT tone — what shows with no artwork and both blur slots faded out.
/// Owning both is what keeps the polarity the blur's: a `Theme.accent` → `Theme.base` pair is
/// bright on a light theme, and so unreadable under the light foreground that arm solves for.
const FLOOR_TONE_START: f64 = 18.0;
const FLOOR_TONE_END: f64 = 8.0;

/// Chroma ceiling for the scrim and the gradient floor. At these tones sRGB gamut-maps almost
/// everything away regardless — a guard against a pathological seed, not a shaping parameter.
const BACKDROP_MAX_CHROMA: f64 = 24.0;

/// WCAG 1.4.11 non-text contrast: icons, the visualizer bars, the stars and the heart carry no
/// linguistic content, so 3:1 is the bar they must clear.
const CHROME_RATIO: f64 = 3.0;
/// WCAG 1.4.3 body-text contrast.
const TEXT_RATIO: f64 = 4.5;

/// Tone band for the hue-carrying chrome tier. The worst permitted backdrop only asks for tone 63,
/// so the floor means the tier cannot regress on any cover; the ceiling stops a very dark sleeve
/// pushing the accent far enough up the scale for gamut mapping to wash the hue out, and —
/// `clamp_to_tone_band` reading it too — stops a near-white *seed* arriving above it.
const CHROME_MIN_TONE: f64 = 70.0;
const CHROME_MAX_TONE: f64 = 92.0;

/// Tone band for primary body text. Both bounds sit above the chrome band's and 4.5:1 is the
/// stricter target, so whenever both tiers are *solved* the title is the brighter. The bands
/// overlap, so a naturally light cover can pass chrome straight through above the solved text tone
/// — bounded by `CHROME_MAX_TONE` rather than closed, closing it meaning a narrower band and a
/// different answer on every cover.
const TEXT_MIN_TONE: f64 = 78.0;
const TEXT_MAX_TONE: f64 = 96.0;

/// Secondary text: the chrome tier's 3:1 target in its own dimmer band, so the two-line hierarchy
/// under the cover survives on every backdrop.
const MUTED_MIN_TONE: f64 = 70.0;
const MUTED_MAX_TONE: f64 = 88.0;

/// Body text reads as near-white carrying a whisper of the album's warmth, not as coloured type —
/// the chrome tier is where the hue gets to be loud.
const TEXT_MAX_CHROMA: f64 = 10.0;
const MUTED_MAX_CHROMA: f64 = 8.0;

// One neutral ink at several weights, a knob per kind of element so tuning one can't drag the
// others. All under 1.0 — the wash reading through is what makes a hueless tier belong to the
// record. Their order is taste and lives only in the values.

/// Icons, stars, the heart, the disc buttons. Everything derived from the tier multiplies this.
const NEUTRAL_CHROME_ALPHA: f32 = 0.85;

/// Chip lettering — held to WCAG's text bar where the glyphs answer to the non-text one, and
/// sitting on a pill rather than on the backdrop.
const AURORA_CHROME_TEXT_ALPHA: f32 = 0.75;

/// The visualizer — the only chrome that animates, and motion carries attention on its own.
const AURORA_VIZ_ALPHA: f32 = 0.70;

/// The pill behind a chip label, as laid on the blur. The disc buttons take it through the globals.
const CHIP_FILL_WEIGHT: f32 = 0.16;

/// The aurora's own. A surface and the label's backing at once, so the one weight that trades
/// against something else's legibility.
const AURORA_CHIP_FILL_ALPHA: f32 = 0.08;

/// Title and the Up-Next song name. Neutral ink rather than `Theme.text`, which carries its
/// palette's cast (Mocha's is blue) and over an album's washes reads as a second hue arguing with
/// them.
const AURORA_TEXT_ALPHA: f32 = 0.85;

/// Its secondary half — artist, album, the lines under them. One colour with the tier above, so
/// this gap is the only hierarchy between the two.
const AURORA_MUTED_ALPHA: f32 = 0.70;

/// Far finer than the tone bands care about, and small enough to live on the stack.
const HISTOGRAM_BINS: usize = 64;

/// Fraction of the brightest pixels the percentile deliberately steps over.
const PERCENTILE_TAIL: f64 = 0.10;

/// [`linearized`] over its whole domain — it takes a `u8`, so there are only 256 answers and each
/// is a `powf(2.4)`, called three times per pixel. The `lab_f` inside [`lstar_from_y`] is left
/// alone: one call rather than three, over a continuous input.
static LINEARIZED: LazyLock<[f64; 256]> = LazyLock::new(|| {
    let mut table = [0.0_f64; 256];
    for (slot, byte) in table.iter_mut().zip(0u8..=u8::MAX) {
        *slot = linearized(byte);
    }
    table
});

/// Relative luminance (Y) of one sRGB pixel.
fn pixel_y(r: u8, g: u8, b: u8) -> f64 {
    // Once rather than per channel: a `LazyLock` deref is a load and a branch.
    let table = &*LINEARIZED;
    let linear = |channel: u8| table[usize::from(channel)];
    0.0722f64.mul_add(linear(b), 0.2126f64.mul_add(linear(r), 0.7152 * linear(g)))
}

/// Perceptual lightness (L*) of one sRGB pixel.
fn pixel_lstar(r: u8, g: u8, b: u8) -> f64 {
    lstar_from_y(pixel_y(r, g, b))
}

/// sRGB transfer function over a *fractional* channel byte → linear 0..100. Same curve as the
/// crate's `linearized`, whose `u8` domain would make the opacity solve below round to a whole
/// byte mid-calculation and quantize its own answer.
fn linear_from_byte(byte: f64) -> f64 {
    let n = (byte / 255.0).clamp(0.0, 1.0);
    if n <= 0.040_449_936 {
        n / 12.92 * 100.0
    } else {
        ((n + 0.055) / 1.055).powf(2.4) * 100.0
    }
}

/// Inverse of [`linear_from_byte`], staying fractional for the same reason.
fn byte_from_linear(linear: f64) -> f64 {
    let n = (linear / 100.0).clamp(0.0, 1.0);
    let encoded = if n <= 0.003_130_8 {
        n * 12.92
    } else {
        1.055f64.mul_add(n.powf(1.0 / 2.4), -0.055)
    };
    encoded * 255.0
}

/// The sRGB grey whose lightness is `tone`, as a channel byte.
fn grey_byte(tone: f64) -> f64 {
    byte_from_linear(y_from_lstar(tone))
}

/// Lightness of a grey channel byte — the inverse of [`grey_byte`].
fn byte_tone(byte: f64) -> f64 {
    lstar_from_y(linear_from_byte(byte))
}

/// Lightness at `bin`'s lower edge.
#[expect(
    clippy::cast_precision_loss,
    reason = "HISTOGRAM_BINS is 64; both operands are exactly representable"
)]
fn bin_floor_lstar(bin: usize) -> f64 {
    bin as f64 / HISTOGRAM_BINS as f64 * 100.0
}

/// Centre lightness of `bin` — an edge would bias the percentile half a bin high.
#[expect(
    clippy::cast_precision_loss,
    reason = "HISTOGRAM_BINS is 64; both operands are exactly representable"
)]
fn bin_centre(bin: usize) -> f64 {
    bin_floor_lstar(bin) + 50.0 / HISTOGRAM_BINS as f64
}

/// Every bin's lower edge in linear Y, so [`luma_p90`] can bin a pixel without converting it.
///
/// L\* is monotone in Y, so the bin a pixel lands in is the same question either way — and asking
/// it here costs a handful of comparisons against the `cbrt` inside [`lstar_from_y`], which was the
/// last transcendental left in that per-pixel loop. The first bin has no floor to compare against,
/// hence one entry short.
static BIN_FLOOR_Y: LazyLock<[f64; HISTOGRAM_BINS - 1]> =
    LazyLock::new(|| std::array::from_fn(|slot| y_from_lstar(bin_floor_lstar(slot + 1))));

/// Histogram bin holding a pixel of luminance `y`.
fn bin_of_y(y: f64) -> usize {
    BIN_FLOOR_Y.partition_point(|floor| y >= *floor)
}

/// The 90th-percentile lightness of a decoded cover, over tightly-packed RGB8.
///
/// **Not the mean, and that is the whole point.** A mostly-black sleeve with a white wordmark has
/// a low mean — "dark backdrop, brighten the chrome" — while the region the title sits on is
/// near-white, a heavy blur smearing that wordmark into a large mid-bright blob rather than
/// averaging it away. Sizing the scrim against the bright *regions* is what keeps one global
/// decision honest on a non-uniform backdrop.
fn luma_p90(rgb: &[u8]) -> Option<f64> {
    if rgb.is_empty() {
        return None;
    }

    let mut histogram = [0u32; HISTOGRAM_BINS];
    let mut total = 0u32;
    for px in rgb.chunks_exact(3) {
        histogram[bin_of_y(pixel_y(px[0], px[1], px[2]))] += 1;
        total += 1;
    }
    if total == 0 {
        return None;
    }

    // Down from the brightest bin: the one where the count crosses the tail is it.
    let tail = (f64::from(total) * PERCENTILE_TAIL).max(1.0);
    let mut seen = 0f64;
    for (bin, count) in histogram.iter().enumerate().rev() {
        seen += f64::from(*count);
        if seen >= tail {
            return Some(bin_centre(bin));
        }
    }
    // Unreachable while `total > 0`, but a darkest-bin fallback beats an `unreachable!`.
    Some(bin_centre(0))
}

/// How many colours a backdrop asks the quantizer for.
///
/// **Deliberately one more than [`crate::ui::aurora::WASH_COUNT`] paints**, which is where that
/// half of the argument lives: median cut splits to a *target*, so asking for three is a different
/// set of boxes rather than this one minus its last, and the washes take the top three of a
/// four-box cut. Four rather than more because each extra box is a finer split of a region already
/// represented, so a fifth colour has nowhere of its own to sit and only dilutes a neighbour.
pub(crate) const SEED_COUNT: usize = 4;

/// Which of the two backdrops a foreground will sit on — and therefore which of two unrelated
/// answers [`BackdropSample::solve`] gives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BackdropKind {
    /// The blurred cover. The brightness is that photograph's, so it is measured and a scrim
    /// drives the composite down into range; [`solve`] is that whole path.
    Blur,
    /// [`crate::ui::aurora`]'s three sweeps over `Theme.base`. There are no solved tones here at
    /// all, and no bound over the washes — the tiers are the theme's own.
    Aurora,
}

/// Everything a solve needs off a decoded cover. All are `None` when there is none — no artwork,
/// or a failed decode — and both arms fall back to the live `Theme.accent`: the blur seeds its floor
/// from that hue at [`floor_luma`], the aurora seats it as a wash pair through
/// [`crate::ui::aurora::tints`].
#[derive(Clone, Copy, Default)]
pub(crate) struct BackdropSample {
    /// Most prominent colour quantized out of the cover, supplying the *hue* for every colour
    /// [`solve`] returns. Always `seeds[0]`, which keeps this tier and the washes on one hue
    /// family.
    pub(crate) accent_argb: Option<u32>,
    /// The same quantize's list, most prominent first, and short whenever the artwork had fewer
    /// regions than that. `ui::aurora` owns the filling rule.
    pub(crate) seeds: [Option<u32>; SEED_COUNT],
    /// [`luma_p90`] of the buffer the scrim gets painted over, which is not the one the seeds came
    /// off — [`Self::measure`] argues why. `None` throughout on the aurora arm, which sizes no
    /// scrim and so never asks.
    pub(crate) luma: Option<f64>,
}

impl BackdropSample {
    /// The colours a decoded cover is made of. Belongs in the `spawn_blocking` task that decoded
    /// it — the buffer is already there.
    ///
    /// **Tightly-packed RGB8, not a pixel buffer**, so a caller hands over whichever form it holds
    /// — `SharedPixelBuffer::as_bytes` or `RgbImage::as_raw` — rather than copying one into the
    /// other to be let in.
    ///
    /// **Hand it a sharp buffer, never a blurred one.** Blur averages the cover's regions into
    /// each other, which is exactly what median cut is looking for.
    ///
    /// An empty `accent_argb` means there was no buffer at all, so [`Self::solve`]'s
    /// `Theme.accent` path is for a missing cover and never a monochrome one — a greyscale sleeve
    /// answers with its own greys.
    pub(crate) fn quantize(cover: &[u8]) -> Self {
        let mut seeds = [None; SEED_COUNT];
        for (slot, seed) in seeds.iter_mut().zip(population_seeds(cover, SEED_COUNT)) {
            *slot = Some(seed);
        }

        Self {
            accent_argb: seeds[0],
            seeds,
            luma: None,
        }
    }

    /// [`Self::quantize`] plus the brightness a scrim is solved against — **the blur arm's, and
    /// nothing else's**. [`theme_backdrop`] answers off the theme's own tokens, so the aurora
    /// reads `luma` nowhere, and the percentile walks every pixel where the quantizer strides them.
    ///
    /// **Two buffers, because the halves want different ones.** Median cut wants `sharp` — blur
    /// averages the regions it is looking for into each other. The percentile wants whatever is
    /// actually *drawn*: [`scrim_alpha`] solves the composite onto [`TARGET_BACKDROP_TONE`], so
    /// measuring anything but the painted layer lands the surface somewhere else entirely. They
    /// are the same buffer only where the caller paints what it measured. Both are RGB8 bytes on
    /// [`Self::quantize`]'s contract.
    pub(crate) fn measure(sharp: &[u8], painted: &[u8]) -> Self {
        Self {
            luma: luma_p90(painted),
            ..Self::quantize(sharp)
        }
    }

    /// The whole colour set for whichever surface is mounted.
    ///
    /// The blur borrows only the *hue* from `theme`, its accent standing in when there was no
    /// artwork so a missing cover doesn't strand the surface on the previous one's colour; [`solve`]
    /// owns every tone. The aurora consults the measurement for nothing at all: [`theme_backdrop`]
    /// answers, and what the cover was reaches the surface only through the washes.
    ///
    /// **The setting is the whole of the branch.** An entry with no artwork used to keep the blur
    /// under either, the aurora's only fallback then being a set of fills with no seed behind them;
    /// `tints` now seats the accent as a wash pair, so every hero has an aurora to paint.
    pub(crate) fn solve(self, theme: &ThemeTokens, kind: BackdropKind) -> BackdropColors {
        match kind {
            BackdropKind::Aurora => theme_backdrop(theme),
            BackdropKind::Blur => solve(
                self.accent_argb.unwrap_or(theme.accent),
                self.luma.unwrap_or_else(floor_luma),
            ),
        }
    }
}

/// Lightness the gradient floor presents when an entry has no artwork — a known quantity rather
/// than a measurement, both stops being ours, which is what lets the artwork-less path run through
/// the *same* solve as every cover.
fn floor_luma() -> f64 {
    gradient_luma_lstar(FLOOR_TONE_START, FLOOR_TONE_END)
}

/// Lightness of a two-stop gradient whose stops are given as sRGB. The Genre hero has no artwork
/// and no floor of ours either, painting a name-hashed gradient (`ui::genres::color`); measuring
/// its stops is what keeps it on the same solve as every cover rather than special-cased into a
/// fixed scrim.
pub(crate) fn gradient_luma(start_rgb: u32, end_rgb: u32) -> f64 {
    gradient_luma_lstar(rgb_lstar(start_rgb), rgb_lstar(end_rgb))
}

/// Midpoint lightness of two stops, averaged in **linear** Y rather than L\* — averaging
/// perceptual lightness understates a gradient between a very dark and a very bright stop, the
/// case the scrim most needs to get right.
fn gradient_luma_lstar(start_lstar: f64, end_lstar: f64) -> f64 {
    lstar_from_y(f64::midpoint(y_from_lstar(start_lstar), y_from_lstar(end_lstar)))
}

/// Perceptual lightness of a packed `0x00RR_GGBB` colour.
fn rgb_lstar(rgb: u32) -> f64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "each shift-and-mask isolates one byte"
    )]
    pixel_lstar((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Scrim opacity that lands `backdrop_luma` on [`TARGET_BACKDROP_TONE`].
///
/// Closed form rather than a search: the renderer composites in gamma space
/// (`c = α·scrim + (1−α)·backdrop` on the encoded bytes) and lightness is monotone in the byte, so
/// with `g` the backdrop's grey byte, `s` the scrim's and `t` the target's, `α = (g − t) / (g − s)`.
/// The scrim's residual chroma makes it an approximation on the order of a tenth of an L*.
fn scrim_alpha(backdrop_luma: f64) -> f32 {
    let g = grey_byte(backdrop_luma);
    let t = grey_byte(TARGET_BACKDROP_TONE);
    let s = grey_byte(SCRIM_TONE);

    // Already at or below target: nothing to darken, take the floor.
    if g <= t {
        return SCRIM_ALPHA_MIN;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an opacity in 0..=1 is exactly representable in f32"
    )]
    let raw = ((g - t) / (g - s)) as f32;
    let snapped = (raw / SCRIM_ALPHA_STEP).round() * SCRIM_ALPHA_STEP;
    snapped.clamp(SCRIM_ALPHA_MIN, SCRIM_ALPHA_MAX)
}

/// Lightness a backdrop presents once the scrim is painted over it — this, not the raw
/// measurement, is what the foreground tiers below are solved against.
fn composited_tone(backdrop_luma: f64, alpha: f32) -> f64 {
    let a = f64::from(alpha);
    let composited = a.mul_add(grey_byte(SCRIM_TONE), (1.0 - a) * grey_byte(backdrop_luma));
    byte_tone(composited)
}

/// Lowest tone above `backdrop_tone` that reaches `ratio`, clamped into `min..=max`.
///
/// `contrast::lighter` inverts the WCAG ratio algebraically. It answers `-1.0` when the ratio is
/// unreachable, which can only happen if the scrim under-darkened, and the band floor is the right
/// degradation there.
fn solve_tone(backdrop_tone: f64, ratio: f64, min: f64, max: f64) -> f64 {
    let wanted = contrast::lighter(backdrop_tone, ratio);
    if wanted < 0.0 {
        return min;
    }
    wanted.clamp(min, max)
}

/// Tone for the hue-carrying chrome tier at [`CHROME_RATIO`].
fn chrome_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, CHROME_RATIO, CHROME_MIN_TONE, CHROME_MAX_TONE)
}

/// Tone for primary body text at [`TEXT_RATIO`].
fn text_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, TEXT_RATIO, TEXT_MIN_TONE, TEXT_MAX_TONE)
}

/// Tone for secondary text at [`CHROME_RATIO`], in its own dimmer band.
fn muted_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, CHROME_RATIO, MUTED_MIN_TONE, MUTED_MAX_TONE)
}

/// Every artwork-derived colour the Now Playing view paints, solved together so they can't drift
/// apart. Plain `u32` RGB — the caller packs them into Slint brushes.
pub(crate) struct BackdropColors {
    /// Scrim fill, alpha in `scrim_alpha`.
    pub scrim: u32,
    pub scrim_alpha: f32,
    /// Gradient-floor stops, shown when the track has no artwork.
    pub floor_start: u32,
    pub floor_end: u32,
    /// Visualizer, stars, heart, chips, buttons. Hue-carrying on the blur, neutral on the aurora.
    pub chrome: u32,
    /// Alpha `chrome` is laid on at — 1.0 on the blur, where the tier *is* the colour, and
    /// [`NEUTRAL_CHROME_ALPHA`] on the aurora, where the wash under it supplies the colour.
    pub chrome_alpha: f32,
    /// Alpha for the lettering on that tier — a different question from the glyphs, WCAG holding
    /// text to [`TEXT_RATIO`] where non-text chrome answers to [`CHROME_RATIO`].
    pub chrome_text_alpha: f32,
    /// Alpha for the pill behind that lettering.
    pub chip_fill_alpha: f32,
    /// Alpha for the visualizer, off the same colour.
    pub viz_alpha: f32,
    /// Fill and glyph of a coverless hero square, always opaque — the mounts take their own weight
    /// off it. `chrome`'s value on the blur and a tier of its own only on the aurora, where that one
    /// is a neutral ink: correct for a chip or a disc the wash reads through, and a pale slab at
    /// 140 px. The theme accent instead, which is what every grid card and track row already paints
    /// behind the same glyph.
    pub placeholder: u32,
    /// Primary body text.
    pub text: u32,
    /// Secondary body text.
    pub muted: u32,
    /// Alphas for the two above — 1.0 on the blur, where each is a tone solved per cover, and
    /// partial on the aurora, where both are the same neutral ink the washes colour.
    pub text_alpha: f32,
    pub muted_alpha: f32,
}

/// Solve the blur's whole set from one seed hue and one backdrop measurement.
///
/// Reach for [`BackdropSample::solve`] rather than calling this directly — it resolves both
/// fallbacks in one place *and* picks the arm, which is what keeps the two consumers from
/// drifting. Genre Detail's procedural gradient is the sole caller here, having stops of its own
/// to measure where a sample has a cover.
pub(crate) fn solve(seed_argb: u32, backdrop_luma: f64) -> BackdropColors {
    let alpha = scrim_alpha(backdrop_luma);
    let tone = composited_tone(backdrop_luma, alpha);
    // *Clamps* rather than sets: a cover already brighter than the solve asks for keeps its own
    // tone, and so its own chroma, up to `CHROME_MAX_TONE`.
    let chrome = clamp_to_tone_band(seed_argb, chrome_tone(tone), CHROME_MAX_TONE);

    BackdropColors {
        scrim: to_tone_capped_chroma(seed_argb, SCRIM_TONE, BACKDROP_MAX_CHROMA),
        scrim_alpha: alpha,
        floor_start: to_tone_capped_chroma(seed_argb, FLOOR_TONE_START, BACKDROP_MAX_CHROMA),
        floor_end: to_tone_capped_chroma(seed_argb, FLOOR_TONE_END, BACKDROP_MAX_CHROMA),
        chrome,
        chrome_alpha: 1.0,
        chrome_text_alpha: 1.0,
        chip_fill_alpha: CHIP_FILL_WEIGHT,
        viz_alpha: 1.0,
        // This tier is hue-carrying and solved against the floor already, so the placeholder has
        // nothing of its own to want here.
        placeholder: chrome,
        text: to_tone_capped_chroma(seed_argb, text_tone(tone), TEXT_MAX_CHROMA),
        muted: to_tone_capped_chroma(seed_argb, muted_tone(tone), MUTED_MAX_CHROMA),
        text_alpha: 1.0,
        muted_alpha: 1.0,
    }
}

/// The aurora's tier set: the theme's own colours, and a neutral chrome the washes read through.
///
/// A theme is a constant where a cover is not, so this needs no solve, and nothing bounds the
/// washes over it — [`crate::ui::aurora::tints`] argues what that trade buys. Reachable past
/// [`BackdropSample::solve`] for the one hero that has washes without a sample to solve from,
/// Genre Detail.
///
/// **The chrome carries no hue of its own**, the answer that survived two attempts at a derived
/// one: a colour taken from the artwork argues with the washes rather than belonging to them, the
/// surface being three of them and any single seed at best one. Black or white at
/// [`NEUTRAL_CHROME_ALPHA`] takes its colour from whichever wash it sits on, so it is right
/// everywhere at once — and with nothing bounding the washes it carries the readable half.
///
/// **The floor is flat**, both stops `base`: a ramp under three sweeps of its own adds a fourth
/// direction to a surface whose whole structure is which way each sweep runs. The scrim is `base`
/// at the alpha floor — nothing mounts the blur stack on this arm, and base over base is inert
/// even if something did.
pub(crate) fn theme_backdrop(theme: &ThemeTokens) -> BackdropColors {
    // One ink for all three tiers below — they differ only in the weight it is laid on at.
    let ink = neutral_ink(theme);

    BackdropColors {
        scrim: theme.base,
        scrim_alpha: SCRIM_ALPHA_MIN,
        floor_start: theme.base,
        floor_end: theme.base,
        chrome: ink,
        chrome_alpha: NEUTRAL_CHROME_ALPHA,
        chrome_text_alpha: AURORA_CHROME_TEXT_ALPHA,
        chip_fill_alpha: AURORA_CHIP_FILL_ALPHA,
        viz_alpha: AURORA_VIZ_ALPHA,
        // The one tier the neutral ink is wrong for — see `BackdropColors::placeholder`. Raw rather
        // than seated like the wash under it: the glyph has to come off that wash, and the accent's
        // own tone is the distance.
        placeholder: theme.accent,
        text: ink,
        muted: ink,
        text_alpha: AURORA_TEXT_ALPHA,
        muted_alpha: AURORA_MUTED_ALPHA,
    }
}

/// The set a band publishes when **no hero is painting** — before the first one opens, and after
/// every teardown.
///
/// Neither arm can spell this through [`BackdropSample::solve`], whose empty sample is an art-less
/// *hero*: both its arms reach for `Theme.accent`, the blur to seed its floor and the aurora to seat
/// a wash pair. That is the honest answer for a hero that has opened with nothing to quantize, and
/// the wrong one for a band still waiting — which wore it for the length of a collage compose.
///
/// **Both arms floor on `Theme.base`, and only the aurora can paint it at its own lightness.** The
/// blur takes it as the *seed* of the floor it always draws — the theme's cast at
/// [`FLOOR_TONE_START`]'s tones — because that arm solves a light foreground against a backdrop it
/// drives down to [`TARGET_BACKDROP_TONE`] and paints its scrim whether or not a cover has landed.
/// Handed the base at its own tone a light theme would answer with a near-full scrim and come back
/// dark anyway, having spent the ink's polarity on the round trip. A neutral seed is the other
/// near-miss: it holds the tones and drops the theme, so the band reads as grey beside the surface
/// it is supposed to be part of.
pub(crate) fn idle_backdrop(theme: &ThemeTokens, kind: BackdropKind) -> BackdropColors {
    match kind {
        BackdropKind::Aurora => theme_backdrop(theme),
        BackdropKind::Blur => solve(theme.base, floor_luma()),
    }
}

/// White on a dark theme, black on a light one.
///
/// Off the *relationship* between base and ink rather than a threshold on either — the same reason
/// `Theme.is-light` exists, two of the six palettes being generated at runtime with no variant id
/// to match on.
fn neutral_ink(theme: &ThemeTokens) -> u32 {
    if rgb_lstar(theme.text) > rgb_lstar(theme.base) {
        0x00ff_ffff
    } else {
        0x0000_0000
    }
}

/// Which backdrop is painted, and the only place that asks — so no publisher can solve for one
/// surface while the mount paints the other.
pub(crate) fn kind(ui: &AppWindow) -> BackdropKind {
    if ui.global::<ThemeGlobal>().get_aurora_backdrop() {
        BackdropKind::Aurora
    } else {
        BackdropKind::Blur
    }
}

/// The blur half a tier builds, or `None` when the aurora is mounted and nothing paints one.
///
/// Beside [`kind`] rather than at each tier so the branch is written once — the setting is
/// restart-gated, so this is asked exactly as often as a tier is constructed.
pub(crate) fn blur_spec(ui: &AppWindow, tier: BlurSpec) -> Option<BlurSpec> {
    match kind(ui) {
        BackdropKind::Blur => Some(tier),
        BackdropKind::Aurora => None,
    }
}

/// The live theme's own colours, packed `0x00RR_GGBB`.
///
/// Read in one place rather than at each publisher so the hero and Now Playing tiers can't
/// disagree about what the theme is — and read at *solve* time rather than cached, a palette
/// change reaching an already-open surface only on its next open either way.
pub(crate) struct ThemeTokens {
    /// The surface the aurora is washed over. Flat — `theme_backdrop` publishes it as both floor
    /// stops, so `Theme.mantle` is nothing this solve needs.
    pub base: u32,
    /// Primary ink.
    pub text: u32,
    /// The hue-carrying tier, and the hue [`BackdropSample::solve`]'s blur arm falls back to for
    /// an entry with no artwork.
    pub accent: u32,
}

pub(crate) fn theme_tokens(ui: &AppWindow) -> ThemeTokens {
    let theme = ui.global::<ThemeGlobal>();
    ThemeTokens {
        base: brush_to_rgb(&theme.get_base()),
        text: brush_to_rgb(&theme.get_text()),
        accent: brush_to_rgb(&theme.get_accent()),
    }
}

/// The scrim as a Slint brush, opacity in the alpha channel — the one tier whose alpha is solved
/// per cover rather than set as a literal.
pub(crate) fn scrim_brush(colors: &BackdropColors) -> Brush {
    tier(colors.scrim, colors.scrim_alpha)
}

/// The chrome tier as a Slint brush. Opaque on the blur; on the aurora the alpha is the whole
/// mechanism, the wash beneath supplying the colour a neutral ink doesn't carry.
pub(crate) fn chrome_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.chrome_alpha)
}

/// The same tier at the weight its *lettering* wants. See [`BackdropColors::chrome_text_alpha`].
pub(crate) fn chrome_text_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.chrome_text_alpha)
}

/// The same tier at the weight the pill behind that lettering wants.
pub(crate) fn chip_fill_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.chip_fill_alpha)
}

/// The same tier at the visualizer's weight.
pub(crate) fn viz_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.viz_alpha)
}

/// A coverless hero square's fill and glyph. Opaque, the mounts taking their own weight off it with
/// `transparentize` — so the fill stays a fraction of whatever this is.
pub(crate) fn placeholder_brush(colors: &BackdropColors) -> Brush {
    brush(colors.placeholder)
}

/// Primary and secondary body text. Opaque on the blur, where each is its own solved tone; one
/// neutral ink at two weights on the aurora, the wash supplying what the ink doesn't carry.
pub(crate) fn text_brush(colors: &BackdropColors) -> Brush {
    tier(colors.text, colors.text_alpha)
}

pub(crate) fn muted_brush(colors: &BackdropColors) -> Brush {
    tier(colors.muted, colors.muted_alpha)
}

/// One colour at one of its weights — a tier differs only in how much wash reads through.
fn chrome_at(colors: &BackdropColors, alpha: f32) -> Brush {
    tier(colors.chrome, alpha)
}

fn tier(rgb: u32, alpha: f32) -> Brush {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "literal alphas sit in 0..=1, and the solved scrim is clamped by `scrim_alpha`"
    )]
    let alpha = (alpha * 255.0).round() as u8;
    brush_with_alpha(rgb, alpha)
}

#[cfg(test)]
#[path = "tests/backdrop_tests.rs"]
mod tests;
