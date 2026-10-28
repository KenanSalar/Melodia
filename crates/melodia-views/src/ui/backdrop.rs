//! How bright an artwork-derived backdrop is, and which colours survive on it.
//!
//! [`BackdropKind`] is where the blur and the aurora part, and they share nothing past it. The
//! blur is a photograph of any brightness, so [`scrim_alpha`] first drives the composite into a
//! known dark band and each foreground tier then solves against that for a WCAG target. Adapting
//! the foreground instead flips polarity on a bright cover, and no one foreground suits a blur
//! bright in one corner and dark in another. The measurement comes off the decoded cover, never the
//! rendered frame, whose chrome tints off the same accent.
//!
//! The aurora neither measures nor bounds; [`crate::ui::aurora`] argues what that buys.

use std::sync::LazyLock;

use material_colors::color::{linearized, lstar_from_y, y_from_lstar};
use material_colors::contrast;
use slint::{Brush, ComponentHandle};

use crate::ui::appearance::rgb::{brush, brush_to_rgb, brush_with_alpha};
use crate::ui::appearance::theme_paint;
use crate::ui::artwork_cache::BlurSpec;
use melodia_artwork::media::image::material_you::{
    clamp_to_tone_band, population_seeds, to_tone_capped_chroma,
};
use melodia_ui::{AppWindow, Theme as ThemeGlobal};

/// HCT tone the composited blur is driven down to, where a light chrome tone clears 3:1 and body
/// text 4.5:1 with margin. Higher shows more artwork for less headroom.
pub(crate) const TARGET_BACKDROP_TONE: f64 = 32.0;

/// Light on purpose: below the target there is nothing to darken, so a black sleeve shows more.
const SCRIM_ALPHA_MIN: f32 = 0.30;

/// Keeps a pathological cover from flattening into a plain dark rectangle.
const SCRIM_ALPHA_MAX: f32 = 0.82;

/// Keeps two covers of one album on one value rather than shimmering as the user skips.
const SCRIM_ALPHA_STEP: f32 = 0.01;

/// Album-tinted near-black. sRGB holds almost no chroma this low, so the hue can't move the solve.
const SCRIM_TONE: f64 = 8.0;

/// The blur's floor with no artwork, in HCT tone. Our own tones keep the blur's polarity, where a
/// `Theme.accent` → `Theme.base` pair is bright on a light theme.
const FLOOR_TONE_START: f64 = 18.0;
const FLOOR_TONE_END: f64 = 8.0;

/// A guard against a pathological seed; at these tones sRGB gamut-maps nearly everything away.
const BACKDROP_MAX_CHROMA: f64 = 24.0;

/// WCAG 1.4.11 non-text contrast, for icons, the visualizer, the stars and the heart.
const CHROME_RATIO: f64 = 3.0;
/// WCAG 1.4.3 body-text contrast.
const TEXT_RATIO: f64 = 4.5;

/// The chrome tier's band. The floor sits above what the worst permitted backdrop asks for. The
/// ceiling keeps a dark sleeve from pushing the accent high enough to wash its hue out, and,
/// through `clamp_to_tone_band`, caps a near-white seed.
const CHROME_MIN_TONE: f64 = 70.0;
const CHROME_MAX_TONE: f64 = 92.0;

/// Body text's band, above the chrome band so a solved title is the brighter. They overlap on
/// purpose: a light cover passes chrome through above the text tone, up to `CHROME_MAX_TONE`.
const TEXT_MIN_TONE: f64 = 78.0;
const TEXT_MAX_TONE: f64 = 96.0;

/// Secondary text at the chrome ratio, in a dimmer band so the two lines keep their order.
const MUTED_MIN_TONE: f64 = 70.0;
const MUTED_MAX_TONE: f64 = 88.0;

/// Near-white with a whisper of the album's warmth; the chrome tier is where the hue gets loud.
const TEXT_MAX_CHROMA: f64 = 10.0;
const MUTED_MAX_CHROMA: f64 = 8.0;

// The aurora's one neutral ink, at a weight per kind of element. All under 1.0: the wash reading
// through is what makes a hueless tier belong to the record.

/// Icons, stars, the heart, the disc buttons.
const NEUTRAL_CHROME_ALPHA: f32 = 0.85;

/// Chip lettering, held to the text bar where the glyphs answer to the non-text one.
const AURORA_CHROME_TEXT_ALPHA: f32 = 0.75;

/// The visualizer, whose motion carries attention on its own.
const AURORA_VIZ_ALPHA: f32 = 0.70;

/// The pill behind a chip label on the blur.
const CHIP_FILL_WEIGHT: f32 = 0.16;

/// The aurora's chip pill, the one weight traded against a label's legibility.
const AURORA_CHIP_FILL_ALPHA: f32 = 0.08;

/// Title ink: neutral rather than `Theme.text`, whose palette cast argues with the washes.
const AURORA_TEXT_ALPHA: f32 = 0.85;

/// The lines under the title, one colour with it, so this gap is their whole hierarchy.
const AURORA_MUTED_ALPHA: f32 = 0.70;

/// Far finer than the tone bands care about, and small enough to live on the stack.
const HISTOGRAM_BINS: usize = 64;

/// Fraction of the brightest pixels the percentile steps over.
const PERCENTILE_TAIL: f64 = 0.10;

/// [`linearized`] over its whole `u8` domain, rather than a `powf` three times per pixel.
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

fn pixel_lstar(r: u8, g: u8, b: u8) -> f64 {
    lstar_from_y(pixel_y(r, g, b))
}

/// The sRGB transfer function over a fractional byte, linear 0..100. The crate's `u8`
/// `linearized` would round the opacity solve mid-calculation.
fn linear_from_byte(byte: f64) -> f64 {
    let n = (byte / 255.0).clamp(0.0, 1.0);
    if n <= 0.040_449_936 { n / 12.92 * 100.0 } else { ((n + 0.055) / 1.055).powf(2.4) * 100.0 }
}

/// Inverse of [`linear_from_byte`].
fn byte_from_linear(linear: f64) -> f64 {
    let n = (linear / 100.0).clamp(0.0, 1.0);
    let encoded =
        if n <= 0.003_130_8 { n * 12.92 } else { 1.055f64.mul_add(n.powf(1.0 / 2.4), -0.055) };
    encoded * 255.0
}

/// The sRGB grey whose lightness is `tone`, as a channel byte.
fn grey_byte(tone: f64) -> f64 {
    byte_from_linear(y_from_lstar(tone))
}

fn byte_tone(byte: f64) -> f64 {
    lstar_from_y(linear_from_byte(byte))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "HISTOGRAM_BINS is 64; both operands are exactly representable"
)]
fn bin_floor_lstar(bin: usize) -> f64 {
    bin as f64 / HISTOGRAM_BINS as f64 * 100.0
}

/// Centre lightness of `bin`; an edge would bias the percentile half a bin high.
#[expect(
    clippy::cast_precision_loss,
    reason = "HISTOGRAM_BINS is 64; both operands are exactly representable"
)]
fn bin_centre(bin: usize) -> f64 {
    bin_floor_lstar(bin) + 50.0 / HISTOGRAM_BINS as f64
}

/// Every bin's lower edge in linear Y, so a pixel bins without [`lstar_from_y`]'s `cbrt`. One
/// short, the first bin having no floor.
static BIN_FLOOR_Y: LazyLock<[f64; HISTOGRAM_BINS - 1]> =
    LazyLock::new(|| std::array::from_fn(|slot| y_from_lstar(bin_floor_lstar(slot + 1))));

fn bin_of_y(y: f64) -> usize {
    BIN_FLOOR_Y.partition_point(|floor| y >= *floor)
}

/// The 90th-percentile lightness of a decoded RGB8 cover.
///
/// Not the mean: a black sleeve with a white wordmark has a low mean while the blur smears that
/// wordmark into the bright blob the title sits on.
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

    // Down from the brightest bin to the one where the count crosses the tail.
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

/// How many colours a backdrop asks the quantizer for: one more than the washes paint, since
/// median cut splits to a target and the washes take the top three of a four-box cut. A fifth
/// would only dilute a neighbour.
pub(crate) const SEED_COUNT: usize = 4;

/// Which backdrop a foreground sits on, and so which of two unrelated answers
/// [`BackdropSample::solve`] gives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BackdropKind {
    /// The blurred cover, measured and scrimmed into range by [`solve`].
    Blur,
    /// [`crate::ui::aurora`]'s sweeps over `Theme.base`, under the theme's own tiers.
    Aurora,
}

/// Everything a solve needs off a decoded cover. All `None` with no cover, both arms then falling
/// back to the live `Theme.accent`.
#[derive(Clone, Copy, Default)]
pub(crate) struct BackdropSample {
    /// `seeds[0]`, the hue of every colour [`solve`] returns.
    pub(crate) accent_rgb: Option<u32>,
    /// The quantize's colours, most prominent first; `ui::aurora` fills a short list.
    pub(crate) seeds: [Option<u32>; SEED_COUNT],
    /// [`luma_p90`] of the painted buffer, which [`Self::measure`] argues. `None` on the aurora.
    pub(crate) luma: Option<f64>,
}

impl BackdropSample {
    /// The colours a decoded cover is made of, quantized in the task that decoded it.
    ///
    /// Tightly-packed RGB8, so a caller hands over whatever it holds. Hand it a sharp buffer: blur
    /// averages away the regions median cut looks for. `accent_rgb` is `None` only for a missing
    /// cover, never a monochrome one.
    pub(crate) fn quantize(cover: &[u8]) -> Self {
        let mut seeds = [None; SEED_COUNT];
        for (slot, seed) in seeds.iter_mut().zip(population_seeds(cover, SEED_COUNT)) {
            *slot = Some(seed);
        }

        Self { accent_rgb: seeds[0], seeds, luma: None }
    }

    /// [`Self::quantize`] plus the brightness the blur's scrim solves against. Two buffers: median
    /// cut wants `sharp`, and the percentile wants the layer actually painted, the one the scrim
    /// lands on [`TARGET_BACKDROP_TONE`].
    pub(crate) fn measure(sharp: &[u8], painted: &[u8]) -> Self {
        Self { luma: luma_p90(painted), ..Self::quantize(sharp) }
    }

    /// The colour set for whichever surface is mounted. The blur takes only the hue from `theme`,
    /// its accent standing in for a missing cover; the aurora answers from the theme alone.
    pub(crate) fn solve(self, theme: &ThemeTokens, kind: BackdropKind) -> BackdropColors {
        match kind {
            BackdropKind::Aurora => theme_backdrop(theme),
            BackdropKind::Blur => {
                solve(self.accent_rgb.unwrap_or(theme.accent), self.luma.unwrap_or_else(floor_luma))
            }
        }
    }
}

/// The floor's lightness with no artwork, a known quantity, so the art-less path runs the same
/// solve as a cover.
fn floor_luma() -> f64 {
    gradient_luma_lstar(FLOOR_TONE_START, FLOOR_TONE_END)
}

/// Lightness of a two-stop sRGB gradient, so Genre's name-hashed hero runs the same solve as a
/// cover.
pub(crate) fn gradient_luma(start_rgb: u32, end_rgb: u32) -> f64 {
    gradient_luma_lstar(rgb_lstar(start_rgb), rgb_lstar(end_rgb))
}

/// Midpoint averaged in linear Y: averaging L* understates a dark-to-bright gradient, the case the
/// scrim most needs right.
fn gradient_luma_lstar(start_lstar: f64, end_lstar: f64) -> f64 {
    lstar_from_y(f64::midpoint(y_from_lstar(start_lstar), y_from_lstar(end_lstar)))
}

/// Perceptual lightness of a packed `0x00RR_GGBB` colour.
fn rgb_lstar(rgb: u32) -> f64 {
    #[expect(clippy::cast_possible_truncation, reason = "each shift-and-mask isolates one byte")]
    pixel_lstar((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Scrim opacity that lands `backdrop_luma` on [`TARGET_BACKDROP_TONE`].
///
/// The renderer composites encoded bytes (`c = α·scrim + (1−α)·backdrop`) and lightness is monotone
/// in the byte, so `α = (g − t) / (g − s)` over the backdrop, target and scrim greys.
fn scrim_alpha(backdrop_luma: f64) -> f32 {
    let g = grey_byte(backdrop_luma);
    let t = grey_byte(TARGET_BACKDROP_TONE);
    let s = grey_byte(SCRIM_TONE);

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

/// Lightness under the painted scrim, which the foreground tiers solve against.
fn composited_tone(backdrop_luma: f64, alpha: f32) -> f64 {
    let a = f64::from(alpha);
    let composited = a.mul_add(grey_byte(SCRIM_TONE), (1.0 - a) * grey_byte(backdrop_luma));
    byte_tone(composited)
}

/// Lowest tone above `backdrop_tone` reaching `ratio`, clamped to the band. Unreachable only if the
/// scrim under-darkened, where the band floor is the right degradation.
fn solve_tone(backdrop_tone: f64, ratio: f64, min: f64, max: f64) -> f64 {
    let Some(wanted) = contrast::lighter(backdrop_tone, ratio) else {
        return min;
    };
    wanted.clamp(min, max)
}

fn chrome_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, CHROME_RATIO, CHROME_MIN_TONE, CHROME_MAX_TONE)
}

fn text_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, TEXT_RATIO, TEXT_MIN_TONE, TEXT_MAX_TONE)
}

fn muted_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, CHROME_RATIO, MUTED_MIN_TONE, MUTED_MAX_TONE)
}

/// Every artwork-derived colour the Now Playing view paints, solved together so they can't drift.
pub(crate) struct BackdropColors {
    pub scrim: u32,
    pub scrim_alpha: f32,
    /// Shown when the track has no artwork.
    pub floor_start: u32,
    pub floor_end: u32,
    /// Visualizer, stars, heart, chips, buttons: hue-carrying on the blur, neutral on the aurora.
    pub chrome: u32,
    /// 1.0 on the blur, where the tier is the colour; partial on the aurora, where the wash is.
    pub chrome_alpha: f32,
    /// For lettering, which WCAG holds to [`TEXT_RATIO`] where glyphs answer to [`CHROME_RATIO`].
    pub chrome_text_alpha: f32,
    pub chip_fill_alpha: f32,
    pub viz_alpha: f32,
    /// A coverless hero square's fill and glyph, opaque. `chrome` on the blur; on the aurora the
    /// theme accent every grid card paints behind that glyph, the neutral ink being a pale slab.
    pub placeholder: u32,
    pub text: u32,
    pub muted: u32,
    /// 1.0 on the blur, each a solved tone; partial on the aurora, one ink at two weights.
    pub text_alpha: f32,
    pub muted_alpha: f32,
}

/// The blur's set from one seed hue and one backdrop measurement. Reach for
/// [`BackdropSample::solve`]; Genre Detail, with stops of its own to measure, is the caller here.
pub(crate) fn solve(seed_rgb: u32, backdrop_luma: f64) -> BackdropColors {
    let alpha = scrim_alpha(backdrop_luma);
    let tone = composited_tone(backdrop_luma, alpha);
    // Clamps rather than sets: a cover already brighter than the solve asks keeps its own tone,
    // and so its chroma, up to `CHROME_MAX_TONE`.
    let chrome = clamp_to_tone_band(seed_rgb, chrome_tone(tone), CHROME_MAX_TONE);

    BackdropColors {
        scrim: to_tone_capped_chroma(seed_rgb, SCRIM_TONE, BACKDROP_MAX_CHROMA),
        scrim_alpha: alpha,
        floor_start: to_tone_capped_chroma(seed_rgb, FLOOR_TONE_START, BACKDROP_MAX_CHROMA),
        floor_end: to_tone_capped_chroma(seed_rgb, FLOOR_TONE_END, BACKDROP_MAX_CHROMA),
        chrome,
        chrome_alpha: 1.0,
        chrome_text_alpha: 1.0,
        chip_fill_alpha: CHIP_FILL_WEIGHT,
        viz_alpha: 1.0,
        placeholder: chrome,
        text: to_tone_capped_chroma(seed_rgb, text_tone(tone), TEXT_MAX_CHROMA),
        muted: to_tone_capped_chroma(seed_rgb, muted_tone(tone), MUTED_MAX_CHROMA),
        text_alpha: 1.0,
        muted_alpha: 1.0,
    }
}

/// The aurora's tiers: the theme's own colours and a neutral chrome, with no solve. Reachable past
/// [`BackdropSample::solve`] for Genre Detail, the one hero with washes but no sample.
///
/// The chrome carries no hue, the answer that survived two attempts at a derived one: a seed is at
/// best one of three washes and argues with the others. The floor is flat `base`, a ramp adding a
/// fourth direction to three sweeps.
pub(crate) fn theme_backdrop(theme: &ThemeTokens) -> BackdropColors {
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
        // Raw rather than seated like the wash: the glyph has to read off it.
        placeholder: theme.accent,
        text: ink,
        muted: ink,
        text_alpha: AURORA_TEXT_ALPHA,
        muted_alpha: AURORA_MUTED_ALPHA,
    }
}

/// The set a band publishes while no hero is painting. Not [`BackdropSample::solve`]'s empty
/// sample, an art-less hero that reaches for `Theme.accent`.
///
/// The blur takes `Theme.base` as its floor's seed, not its tone: it scrims whether or not a cover
/// landed, so a light base at its own tone would come back dark anyway.
pub(crate) fn idle_backdrop(theme: &ThemeTokens, kind: BackdropKind) -> BackdropColors {
    match kind {
        BackdropKind::Aurora => theme_backdrop(theme),
        BackdropKind::Blur => solve(theme.base, floor_luma()),
    }
}

/// White on a dark theme, black on a light one, from base against ink rather than a threshold:
/// two palettes are generated with no variant id.
fn neutral_ink(theme: &ThemeTokens) -> u32 {
    if rgb_lstar(theme.text) > rgb_lstar(theme.base) { 0x00ff_ffff } else { 0x0000_0000 }
}

/// Which backdrop is painted. The only place that asks, so no publisher solves for the other.
pub(crate) fn kind(ui: &AppWindow) -> BackdropKind {
    if ui.global::<ThemeGlobal>().get_aurora_backdrop() {
        BackdropKind::Aurora
    } else {
        BackdropKind::Blur
    }
}

/// A tier's blur half, or `None` under the aurora, where nothing paints one.
pub(crate) fn blur_spec(ui: &AppWindow, tier: BlurSpec) -> Option<BlurSpec> {
    match kind(ui) {
        BackdropKind::Blur => Some(tier),
        BackdropKind::Aurora => None,
    }
}

/// The live theme's own colours, packed `0x00RR_GGBB` and read in one place so the hero and Now
/// Playing tiers agree. Off the palette `Theme` is settling on: mid-fade the global holds a blend.
pub(crate) struct ThemeTokens {
    /// The aurora's surface and both floor stops.
    pub base: u32,
    pub text: u32,
    /// The blur's fallback hue for an entry with no artwork.
    pub accent: u32,
}

pub(crate) fn theme_tokens(ui: &AppWindow) -> ThemeTokens {
    if let Some(settled) = theme_paint::settled() {
        let palette = settled.palette;
        return ThemeTokens { base: palette.base, text: palette.text, accent: settled.accent };
    }
    // Nothing painted yet, so the global still holds its declared defaults.
    let theme = ui.global::<ThemeGlobal>();
    ThemeTokens {
        base: brush_to_rgb(&theme.get_base()),
        text: brush_to_rgb(&theme.get_text()),
        accent: brush_to_rgb(&theme.get_accent()),
    }
}

/// The scrim, the one tier whose alpha is solved per cover.
pub(crate) fn scrim_brush(colors: &BackdropColors) -> Brush {
    tier(colors.scrim, colors.scrim_alpha)
}

pub(crate) fn chrome_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.chrome_alpha)
}

pub(crate) fn chrome_text_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.chrome_text_alpha)
}

pub(crate) fn chip_fill_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.chip_fill_alpha)
}

pub(crate) fn viz_brush(colors: &BackdropColors) -> Brush {
    chrome_at(colors, colors.viz_alpha)
}

pub(crate) fn placeholder_brush(colors: &BackdropColors) -> Brush {
    brush(colors.placeholder)
}

pub(crate) fn text_brush(colors: &BackdropColors) -> Brush {
    tier(colors.text, colors.text_alpha)
}

pub(crate) fn muted_brush(colors: &BackdropColors) -> Brush {
    tier(colors.muted, colors.muted_alpha)
}

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
