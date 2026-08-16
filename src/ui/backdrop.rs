//! How bright a blurred-artwork backdrop is, and which colours survive on it.
//!
//! Two surfaces float their chrome directly on a blurred cover, so nothing about their legibility
//! is knowable until the cover is. **Measure** the blur ([`luma_p90`]) and solve a scrim opacity
//! driving the *composited* result into a known dark band ([`scrim_alpha`], [`composited_tone`]),
//! then **solve** each foreground tier's HCT tone against that for a WCAG target.
//!
//! That order is load-bearing. Adapting the foreground first means a bright cover is answered by a
//! *polarity flip* past the black/white crossover rather than a darker accent, and a blurred cover
//! isn't uniform besides, so no global foreground decision serves a backdrop bright in one corner
//! and dark in another. Pin the backdrop and every cover ends up dark, so one light hue-carrying
//! foreground is correct everywhere.
//!
//! **The measurement comes off the decoded blur buffer, never the rendered frame** — the chrome
//! tints itself off the same accent feeding the backdrop, so sampling the composite closes a
//! feedback loop. A consumer borrows only the artwork's *hue* and owns every lightness decision,
//! which is why these surfaces look the same under every theme.

use std::sync::LazyLock;

use material_colors::color::{linearized, lstar_from_y, y_from_lstar};
use material_colors::contrast;
use slint::{Brush, Rgb8Pixel, SharedPixelBuffer};

use crate::services::material_you::{
    clamp_to_tone_band, extract_seeds_from_rgb8, to_tone_capped_chroma,
};
use crate::themes::brush_with_alpha;

/// HCT tone the composited backdrop is driven down to. Below it a light hue-carrying chrome tone
/// clears WCAG's 3:1 non-text bar with margin and body text clears 4.5:1 without washing out.
/// Raising it shows more artwork and costs contrast headroom.
///
/// It is the *composite* that must respect this, not any single layer: [`crate::ui::aurora`]'s
/// tints sit above it and arrive underneath once their alpha is applied.
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
/// Owning both is what keeps the polarity ours: a `Theme.accent` → `Theme.base` pair is bright on
/// a light theme, and so unreadable under the light foreground this module solves for.
const FLOOR_TONE_START: f64 = 18.0;
const FLOOR_TONE_END: f64 = 8.0;

/// Chroma ceiling for the scrim and the gradient floor. At these tones sRGB gamut-maps almost
/// everything away regardless — a guard against a pathological seed, not a shaping parameter.
/// Deliberately not shared with [`crate::ui::aurora`]'s washes, which sit high enough for a
/// ceiling to bind and so answer how loud the album gets rather than what the gamut passes.
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

/// Perceptual lightness (L*) of one sRGB pixel.
fn pixel_lstar(r: u8, g: u8, b: u8) -> f64 {
    // Once rather than per channel: a `LazyLock` deref is a load and a branch.
    let table = &*LINEARIZED;
    let linear = |channel: u8| table[usize::from(channel)];
    let y = 0.0722f64.mul_add(linear(b), 0.2126f64.mul_add(linear(r), 0.7152 * linear(g)));
    lstar_from_y(y)
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

/// Centre lightness of `bin` — an edge would bias the percentile half a bin high.
#[expect(
    clippy::cast_precision_loss,
    reason = "HISTOGRAM_BINS is 64; both operands are exactly representable"
)]
fn bin_centre(bin: usize) -> f64 {
    (bin as f64 + 0.5) / HISTOGRAM_BINS as f64 * 100.0
}

/// The 90th-percentile lightness of a decoded blur buffer.
///
/// **Not the mean, and that is the whole point.** A mostly-black sleeve with a white wordmark has
/// a low mean — "dark backdrop, brighten the chrome" — while the region the title sits on is
/// near-white, a heavy blur smearing that wordmark into a large mid-bright blob rather than
/// averaging it away. Sizing the scrim against the bright *regions* is what keeps one global
/// decision honest on a non-uniform backdrop.
fn luma_p90(buf: &SharedPixelBuffer<Rgb8Pixel>) -> Option<f64> {
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

/// How many hue-separated seeds a backdrop asks the quantizer for — one per aurora wash, so
/// `ui::aurora` has to agree. Three because `Score` typically still holds 60–90° of separation
/// there, where six forces it toward its 15° floor and hands back near-duplicates.
pub(crate) const SEED_COUNT: usize = 3;

/// Everything a solve needs off a decoded cover. All are `None` when there is none — no artwork,
/// or a failed decode — and the publisher falls back to the live `Theme.accent` and
/// [`floor_luma`].
#[derive(Clone, Copy, Default)]
pub(crate) struct BackdropSample {
    /// Dominant colour quantized out of the cover, supplying the *hue* for every colour [`solve`]
    /// returns. Always `seeds[0]`, which keeps this tier and the washes on one hue family.
    pub(crate) accent_argb: Option<u32>,
    /// The same quantize's ranked list, best first, short when the artwork couldn't separate that
    /// many hues. `ui::aurora` owns the filling rule.
    pub(crate) seeds: [Option<u32>; SEED_COUNT],
    /// [`luma_p90`] of the same buffer.
    pub(crate) luma: Option<f64>,
}

impl BackdropSample {
    /// Measure a decoded cover. CPU-bound — the quantize dominates — so it belongs in the
    /// `spawn_blocking` task that decoded it, never on the UI thread.
    ///
    /// **Hand it the sharp downscale, never a blurred one.** Blur averages away exactly the hue
    /// separation `Score` looks for: measured on a real cover, two seeds against three, and the
    /// two nearly a shared hue. Amberol quantizes the cover itself for the same reason.
    ///
    /// An empty `accent_argb` means there was no buffer at all, so [`Self::solve`]'s
    /// `Theme.accent` path is for a missing cover and never a monochrome one — a greyscale sleeve
    /// answers with its own dominant grey.
    pub(crate) fn measure(cover: &SharedPixelBuffer<Rgb8Pixel>) -> Self {
        let mut seeds = [None; SEED_COUNT];
        for (slot, seed) in seeds.iter_mut().zip(extract_seeds_from_rgb8(cover, SEED_COUNT)) {
            *slot = Some(seed);
        }

        Self {
            accent_argb: seeds[0],
            seeds,
            luma: luma_p90(cover),
        }
    }

    /// Solve the whole colour set from this measurement.
    ///
    /// `theme_accent` supplies the hue when there was no artwork, so a missing-artwork entry
    /// doesn't strand the surface on the previous one's colour. Only the hue is borrowed —
    /// [`solve`] owns every tone.
    pub(crate) fn solve(self, theme_accent: u32) -> BackdropColors {
        solve(self.accent_argb.unwrap_or(theme_accent), self.luma.unwrap_or_else(floor_luma))
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
    /// Hue-carrying tier: visualizer, stars, heart, chips, buttons.
    pub chrome: u32,
    /// Primary body text.
    pub text: u32,
    /// Secondary body text.
    pub muted: u32,
}

/// Solve the whole set from one seed hue and one backdrop measurement.
///
/// Reach for [`BackdropSample::solve`] rather than calling this directly — it resolves both
/// fallbacks in one place, which is what keeps the two consumers from drifting. Genre Detail's
/// procedural gradient is the sole caller here, having no artwork.
pub(crate) fn solve(seed_argb: u32, backdrop_luma: f64) -> BackdropColors {
    let alpha = scrim_alpha(backdrop_luma);
    let tone = composited_tone(backdrop_luma, alpha);

    BackdropColors {
        scrim: to_tone_capped_chroma(seed_argb, SCRIM_TONE, BACKDROP_MAX_CHROMA),
        scrim_alpha: alpha,
        floor_start: to_tone_capped_chroma(seed_argb, FLOOR_TONE_START, BACKDROP_MAX_CHROMA),
        floor_end: to_tone_capped_chroma(seed_argb, FLOOR_TONE_END, BACKDROP_MAX_CHROMA),
        // *Clamps* rather than sets: a cover already brighter than the solve asks for keeps its
        // own tone, and so its own chroma, up to `CHROME_MAX_TONE`.
        chrome: clamp_to_tone_band(seed_argb, chrome_tone(tone), CHROME_MAX_TONE),
        text: to_tone_capped_chroma(seed_argb, text_tone(tone), TEXT_MAX_CHROMA),
        muted: to_tone_capped_chroma(seed_argb, muted_tone(tone), MUTED_MAX_CHROMA),
    }
}

/// The scrim as a Slint brush, opacity baked into the alpha channel. Here rather than at each
/// publisher because the bound making the lossy cast safe is [`scrim_alpha`]'s clamp, which
/// neither call site can see.
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
