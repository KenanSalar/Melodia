//! How bright a blurred-artwork backdrop is, and which colours survive on it.
//!
//! Two surfaces float their chrome directly on a blurred cover — the Now
//! Playing view over a full-screen blur, and the detail / Favorites /
//! Recently-Played heroes over a banner-height one — so nothing about their
//! legibility is knowable until the cover is. This module is the whole of that
//! reasoning, in two halves:
//!
//! 1. **Measure** the blurred backdrop ([`luma_p90`]) and solve the scrim
//!    opacity that drives the *composited* result into a known dark band
//!    ([`scrim_alpha`], [`composited_tone`]).
//! 2. **Solve** each foreground tier's HCT tone against that composited tone
//!    for a WCAG contrast target ([`chrome_tone`], [`text_tone`],
//!    [`muted_tone`]).
//!
//! The order is load-bearing. Adapting the foreground *without* first pinning
//! the backdrop cannot work here, and the reason is not aesthetic: over a
//! bright cover the fix is not a darker accent but a *polarity flip* past the
//! black/white crossover, which would throw the album's colour away and
//! reverse itself between tracks. Worse, a blurred cover is not uniform — one
//! global foreground decision cannot serve a backdrop that is bright in one
//! corner and dark in another. Pinning the backdrop first removes both
//! problems: every cover ends up dark, so one light, hue-carrying foreground
//! is correct everywhere. This is what Spotify, Apple Music and Plexamp all
//! do; Android's `MediaStyle` notifications are the one place the industry
//! solves a foreground tone instead, and they do it against a *flat swatch*
//! they control rather than against an image.
//!
//! **The measurement is taken from the decoded blur buffer, never from the
//! rendered frame.** The chrome tints itself off the same accent that feeds
//! the backdrop, so sampling the composite would close a feedback loop.
//!
//! A consumer therefore takes its *hue* from the artwork (falling back to
//! `Theme.accent`) but owns every *lightness* decision itself — which is why
//! these surfaces look the same under Mocha, Latte, macOS Light and Material
//! You.
//!
//! The solve is deliberately consumer-agnostic: it takes one seed colour and
//! one brightness measurement and hands back a whole colour set, so the two
//! call sites ([`crate::ui::now_playing`] and [`crate::ui::hero_backdrop`])
//! share one set of tuning constants rather than drifting apart. Both
//! measurements come off one buffer in one place — [`BackdropSample::measure`]
//! — so a producer can't quantize one blur and measure another.

use std::sync::LazyLock;

use material_colors::color::{linearized, lstar_from_y, y_from_lstar};
use material_colors::contrast;
use slint::{Brush, Rgb8Pixel, SharedPixelBuffer};

use crate::services::material_you::{
    extract_source_argb_from_rgb8, lift_to_min_tone, to_tone_capped_chroma,
};
use crate::themes::brush_with_alpha;

/// HCT tone the composited backdrop is driven down to. Below this a light,
/// hue-carrying chrome tone clears WCAG's 3:1 non-text bar with margin, and
/// body text clears 4.5:1 without having to wash out to white.
///
/// Raising it shows more of the artwork and costs contrast headroom; lowering
/// it buys headroom and dims the cover. 32 is where a pure-white sleeve still
/// leaves the tone-70 accent at 3.8:1 — comfortably above the 1.4:1 the fixed
/// scrim it replaces produced.
const TARGET_BACKDROP_TONE: f64 = 32.0;

/// Floor on the solved scrim opacity. Below the target tone the scrim has no
/// darkening work left to do, and a dark cover has contrast to spare — so this
/// is deliberately *lighter* than the fixed 45% it replaces, and a black
/// sleeve now shows more of itself than it used to.
const SCRIM_ALPHA_MIN: f32 = 0.30;

/// Ceiling on the solved scrim opacity. A pure-white sleeve wants ~0.78; the
/// cap keeps a pathological cover from flattening into a plain dark rectangle.
const SCRIM_ALPHA_MAX: f32 = 0.82;

/// Quantization step for the solved opacity. Two covers from the same album
/// differ by a fraction of a percent, and snapping keeps them on one value
/// rather than shimmering between neighbours as the user skips.
const SCRIM_ALPHA_STEP: f32 = 0.01;

/// HCT tone the accent is driven to for the scrim fill — album-tinted
/// near-black rather than flat black. sRGB holds almost no chroma this low, so
/// the tint is a hint rather than a colour and the opacity solve below is
/// unaffected by which hue it is.
const SCRIM_TONE: f64 = 8.0;

/// Gradient-floor stops, in HCT tone. This is what shows when an entry has no
/// artwork (or its cover failed to decode) and the two blur slots are faded
/// out. Both consumers arrived here from the same starting point —
/// `Theme.accent.mix(Theme.base, 0.2)` → `Theme.base` — which on a light theme
/// is bright, and so unreadable under the light foreground this module solves
/// for. Owning both stops keeps the polarity ours.
const FLOOR_TONE_START: f64 = 18.0;
const FLOOR_TONE_END: f64 = 8.0;

/// Chroma ceiling for the scrim and the gradient floor. At these tones sRGB
/// gamut-maps almost everything away regardless; this is a guard against a
/// pathological seed, not a shaping parameter.
const BACKDROP_MAX_CHROMA: f64 = 24.0;

/// WCAG 1.4.11 non-text contrast: icons, the visualizer bars, the stars and
/// the heart carry no linguistic content, so 3:1 is the bar they must clear.
const CHROME_RATIO: f64 = 3.0;
/// WCAG 1.4.3 body-text contrast.
const TEXT_RATIO: f64 = 4.5;

/// Tone band for the hue-carrying chrome tier.
///
/// The floor is the constant this module inherited from the one-sided fix it
/// replaces: at the worst permitted backdrop the solve only asks for tone 63,
/// so holding the old floor means the chrome tier cannot regress on any cover.
/// The ceiling stops a very dark sleeve from pushing the accent so far up the
/// tone scale that gamut mapping washes the album's hue out of it.
const CHROME_MIN_TONE: f64 = 70.0;
const CHROME_MAX_TONE: f64 = 92.0;

/// Tone band for primary body text. Sits above the chrome band so the title
/// reads as the brightest thing on the backdrop whatever the cover.
const TEXT_MIN_TONE: f64 = 78.0;
const TEXT_MAX_TONE: f64 = 96.0;

/// Tone band for secondary text (artist, album, Up-Next artist / duration).
/// Same 3:1 target as the chrome tier but its own band, so the two-line
/// hierarchy under the cover survives on every backdrop.
const MUTED_MIN_TONE: f64 = 70.0;
const MUTED_MAX_TONE: f64 = 88.0;

/// Chroma ceilings for the two text tiers. Body text wants to read as
/// near-white carrying a whisper of the album's warmth, not as coloured type —
/// the chrome tier is where the hue gets to be loud.
const TEXT_MAX_CHROMA: f64 = 10.0;
const MUTED_MAX_CHROMA: f64 = 8.0;

/// Bin count for the luminance histogram. ~1.6 L* per bin over the 0..100
/// range — far finer than the tone bands above care about, and small enough to
/// live on the stack.
const HISTOGRAM_BINS: usize = 64;

/// Fraction of the brightest pixels the percentile deliberately steps over.
const PERCENTILE_TAIL: f64 = 0.10;

/// [`linearized`] over its whole domain — the sRGB transfer curve takes a `u8`,
/// so there are only 256 answers and each is a `powf(2.4)`.
///
/// [`luma_p90`] calls it three times per pixel across a buffer of tens of
/// thousands, which is most of what that pass spends. 2 KiB, built once. The
/// `lab_f` inside [`lstar_from_y`] is left alone deliberately: it's one call
/// rather than three, its input is a continuous mix of these three so it can't
/// be tabulated the same way, and the obvious alternative — binary-searching
/// precomputed bin edges — trades a cube root for six unpredictable branches.
static LINEARIZED: LazyLock<[f64; 256]> = LazyLock::new(|| {
    let mut table = [0.0_f64; 256];
    for (slot, byte) in table.iter_mut().zip(0u8..=u8::MAX) {
        *slot = linearized(byte);
    }
    table
});

/// Perceptual lightness (L*) of one sRGB pixel.
fn pixel_lstar(r: u8, g: u8, b: u8) -> f64 {
    // Resolved once rather than per channel: a `LazyLock` deref is a load and a
    // branch, and this runs three times a pixel across the whole buffer.
    let table = &*LINEARIZED;
    let linear = |channel: u8| table[usize::from(channel)];
    let y = 0.0722f64.mul_add(
        linear(b),
        0.2126f64.mul_add(linear(r), 0.7152 * linear(g)),
    );
    lstar_from_y(y)
}

/// sRGB transfer function over a *fractional* channel byte → linear 0..100.
///
/// The crate's `linearized` takes a `u8`, which would force the opacity solve
/// below to round to a whole byte mid-calculation and quantize its own answer.
/// This is the same curve in the float domain.
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

/// Histogram bin holding `lstar`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "HISTOGRAM_BINS is 64 and the value is clamped in range before the cast"
)]
fn bin_index(lstar: f64) -> usize {
    (lstar / 100.0 * HISTOGRAM_BINS as f64).clamp(0.0, (HISTOGRAM_BINS - 1) as f64) as usize
}

/// Centre lightness of `bin`. Reporting the centre rather than an edge keeps
/// the percentile estimate from being biased half a bin high.
#[expect(
    clippy::cast_precision_loss,
    reason = "HISTOGRAM_BINS is 64; both operands are exactly representable"
)]
fn bin_centre(bin: usize) -> f64 {
    (bin as f64 + 0.5) / HISTOGRAM_BINS as f64 * 100.0
}

/// The 90th-percentile lightness of a decoded blur buffer.
///
/// **Not the mean, and that is the whole point.** A sleeve that is mostly
/// black with a white wordmark has a low mean — which would say "dark backdrop,
/// brighten the chrome" — while the region the title actually sits on is
/// near-white. Blurring softens that but does not remove it: a heavy blur
/// smears a wordmark into a large mid-bright blob rather than averaging it
/// into the surround. Sizing the scrim against the bright *regions* is what
/// keeps a single global decision honest on a non-uniform backdrop.
///
/// Returns `None` for an empty buffer.
pub(crate) fn luma_p90(buf: &SharedPixelBuffer<Rgb8Pixel>) -> Option<f64> {
    let bytes = buf.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut histogram = [0u32; HISTOGRAM_BINS];
    let mut total = 0u32;
    for px in bytes.chunks_exact(3) {
        histogram[bin_index(pixel_lstar(px[0], px[1], px[2]))] += 1;
        total += 1;
    }
    if total == 0 {
        return None;
    }

    // Walk down from the brightest bin until the accumulated count crosses the
    // tail — that bin holds the percentile.
    let tail = (f64::from(total) * PERCENTILE_TAIL).max(1.0);
    let mut seen = 0f64;
    for (bin, count) in histogram.iter().enumerate().rev() {
        seen += f64::from(*count);
        if seen >= tail {
            return Some(bin_centre(bin));
        }
    }
    // Unreachable while `total > 0` — the loop above consumes every pixel — but
    // a darkest-bin fallback beats an `unreachable!` on a pure function.
    Some(bin_centre(0))
}

/// Everything a solve needs measured off a decoded blur: the hue to seed from
/// and how bright the blur is.
///
/// Both fields are `None` when there is no blur to measure — a track or entity
/// with no artwork, or a decode that failed — and the publisher then falls back
/// to the live `Theme.accent` and [`floor_luma`].
#[derive(Clone, Copy, Default)]
pub(crate) struct BackdropSample {
    /// Dominant colour quantized out of the blur. Supplies the *hue* for every
    /// colour [`solve`] returns.
    pub(crate) accent_argb: Option<u32>,
    /// [`luma_p90`] of the same buffer.
    pub(crate) luma: Option<f64>,
}

impl BackdropSample {
    /// Measure a decoded blur.
    ///
    /// CPU-bound — the quantize dominates, and the percentile pass beside it is
    /// linear over a buffer that small — so this belongs in the `spawn_blocking`
    /// task that produced the blur, never on the UI thread.
    ///
    /// Quantizing the *blur* rather than the sharp source is deliberate: a
    /// downscaled, blurred buffer is plenty of pixels for `QuantizerCelebi` and
    /// re-quantizing the full-size cover costs several times more for no
    /// perceptual gain.
    ///
    /// An empty `accent_argb` means there was no buffer at all. A cover with no
    /// colour above the scorer's chroma cutoff — a greyscale sleeve — answers
    /// the quantizer's *own* fallback hue instead, so the `Theme.accent` path in
    /// [`Self::solve`] is for a missing cover, never a monochrome one.
    pub(crate) fn measure(blur: &SharedPixelBuffer<Rgb8Pixel>) -> Self {
        Self {
            accent_argb: extract_source_argb_from_rgb8(blur),
            luma: luma_p90(blur),
        }
    }

    /// Solve the whole colour set from this measurement.
    ///
    /// `theme_accent` supplies the hue when there was no artwork to take one
    /// from, so a missing-artwork or failed-decode entry doesn't strand the
    /// surface on the previous one's colour and a theme change propagates on the
    /// next open. Only the hue is borrowed — [`solve`] owns every tone, so the
    /// result looks the same whether the theme underneath is light or dark.
    ///
    /// No blur to measure means the gradient floor is what shows, and both of
    /// its stops are ours — so [`floor_luma`] is a known value rather than a
    /// guess, and the artwork-less path runs through the same solve as every
    /// cover.
    pub(crate) fn solve(self, theme_accent: u32) -> BackdropColors {
        solve(
            self.accent_argb.unwrap_or(theme_accent),
            self.luma.unwrap_or_else(floor_luma),
        )
    }
}

/// Lightness the gradient floor presents when an entry has no artwork.
///
/// Both stops are ours ([`FLOOR_TONE_START`] / [`FLOOR_TONE_END`]), so this is
/// a known quantity rather than a measurement — which is what lets the
/// artwork-less path run through the *same* solve as every other cover instead
/// of being special-cased.
pub(crate) fn floor_luma() -> f64 {
    gradient_luma_lstar(FLOOR_TONE_START, FLOOR_TONE_END)
}

/// Lightness of a two-stop gradient whose stops are given as sRGB.
///
/// The Genre hero has no artwork to measure and no floor of ours either — it
/// paints a name-hashed gradient (`ui::genres::color`) that is already
/// theme-independent and deliberately dim. Measuring those stops lets it run
/// through the same solve as every cover rather than being special-cased into
/// a fixed scrim.
pub(crate) fn gradient_luma(start_rgb: u32, end_rgb: u32) -> f64 {
    gradient_luma_lstar(rgb_lstar(start_rgb), rgb_lstar(end_rgb))
}

/// Midpoint lightness of two stops, averaged in **linear** Y rather than in
/// L\*. Averaging perceptual lightness would understate a gradient running
/// between a very dark and a very bright stop, which is the case the scrim
/// most needs to get right.
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
/// (`c = α·scrim + (1−α)·backdrop` on the encoded bytes) and lightness is
/// monotone in the byte, so with `g` the backdrop's grey byte, `s` the scrim's
/// and `t` the target's, `α = (g − t) / (g − s)`. The scrim's residual chroma
/// makes this an approximation on the order of a tenth of an L*.
pub(crate) fn scrim_alpha(backdrop_luma: f64) -> f32 {
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

/// The lightness a backdrop of `backdrop_luma` actually presents once the
/// scrim is painted over it at `alpha`. This — not the raw measurement — is
/// what the foreground tiers below are solved against.
pub(crate) fn composited_tone(backdrop_luma: f64, alpha: f32) -> f64 {
    let a = f64::from(alpha);
    let composited = a.mul_add(grey_byte(SCRIM_TONE), (1.0 - a) * grey_byte(backdrop_luma));
    byte_tone(composited)
}

/// Lowest tone above `backdrop_tone` that reaches `ratio`, clamped into
/// `min..=max`.
///
/// `contrast::lighter` inverts the WCAG ratio algebraically — the analytic
/// form of the 15-iteration LAB binary search AOSP runs for media
/// notifications. It answers `-1.0` when the ratio is unreachable; that can
/// only happen if the scrim under-darkened, and falling back to the band floor
/// is the right degradation.
fn solve_tone(backdrop_tone: f64, ratio: f64, min: f64, max: f64) -> f64 {
    let wanted = contrast::lighter(backdrop_tone, ratio);
    if wanted < 0.0 {
        return min;
    }
    wanted.clamp(min, max)
}

/// Tone for the hue-carrying chrome tier at [`CHROME_RATIO`].
pub(crate) fn chrome_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, CHROME_RATIO, CHROME_MIN_TONE, CHROME_MAX_TONE)
}

/// Tone for primary body text at [`TEXT_RATIO`].
pub(crate) fn text_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, TEXT_RATIO, TEXT_MIN_TONE, TEXT_MAX_TONE)
}

/// Tone for secondary text at [`CHROME_RATIO`], in its own dimmer band.
pub(crate) fn muted_tone(backdrop_tone: f64) -> f64 {
    solve_tone(backdrop_tone, CHROME_RATIO, MUTED_MIN_TONE, MUTED_MAX_TONE)
}

/// Every artwork-derived colour the Now Playing view paints, solved together
/// so they can't drift apart. Plain `u32` RGB — the caller packs them into
/// Slint brushes.
pub(crate) struct BackdropColors {
    /// Scrim fill, alpha in `scrim_alpha`.
    pub scrim: u32,
    pub scrim_alpha: f32,
    /// Gradient-floor stops, shown when the track has no artwork.
    pub floor_start: u32,
    pub floor_end: u32,
    /// Hue-carrying tier: visualizer, stars, heart, chips, buttons.
    pub chrome: u32,
    /// Primary body text.
    pub text: u32,
    /// Secondary body text.
    pub muted: u32,
}

/// Solve the whole set from one seed hue and one backdrop measurement.
///
/// `seed_argb` is the accent quantized out of the artwork, or the live
/// `Theme.accent` when there is none — a consumer borrows the theme's hue but
/// never its lightness. `backdrop_luma` is [`luma_p90`] of the blur, or
/// [`floor_luma`] when there is no blur to measure.
///
/// Reach for [`BackdropSample::solve`] instead of calling this directly: it
/// resolves both fallbacks in one place, which is what keeps the two consumers
/// from drifting. Genre Detail's procedural gradient is the sole caller here,
/// having no artwork to have measured.
pub(crate) fn solve(seed_argb: u32, backdrop_luma: f64) -> BackdropColors {
    let alpha = scrim_alpha(backdrop_luma);
    let tone = composited_tone(backdrop_luma, alpha);

    BackdropColors {
        scrim: to_tone_capped_chroma(seed_argb, SCRIM_TONE, BACKDROP_MAX_CHROMA),
        scrim_alpha: alpha,
        floor_start: to_tone_capped_chroma(seed_argb, FLOOR_TONE_START, BACKDROP_MAX_CHROMA),
        floor_end: to_tone_capped_chroma(seed_argb, FLOOR_TONE_END, BACKDROP_MAX_CHROMA),
        // The chrome tier *lifts* rather than sets: a cover whose accent is
        // already brighter than the solve asks for keeps its own tone, and so
        // its own chroma, instead of being dragged down to the minimum.
        chrome: lift_to_min_tone(seed_argb, chrome_tone(tone)),
        text: to_tone_capped_chroma(seed_argb, text_tone(tone), TEXT_MAX_CHROMA),
        muted: to_tone_capped_chroma(seed_argb, muted_tone(tone), MUTED_MAX_CHROMA),
    }
}

/// The scrim as a Slint brush, opacity baked into the alpha channel.
///
/// Lives here rather than at each publisher because this is the one place a
/// solved `f32` has to survive a lossy cast, and the bound that makes the cast
/// safe is [`scrim_alpha`]'s clamp a few lines up — not something either call
/// site can see.
pub(crate) fn scrim_brush(colors: &BackdropColors) -> Brush {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "solved alpha is clamped to 0..=1 by `scrim_alpha`"
    )]
    let alpha = (colors.scrim_alpha * 255.0).round() as u8;
    brush_with_alpha(colors.scrim, alpha)
}

#[cfg(test)]
#[path = "tests/backdrop_tests.rs"]
mod tests;
