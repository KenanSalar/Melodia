//! The colours an aurora backdrop washes over its base, from the artwork's own seeds.
//!
//! Each wash is the quantizer's answer untouched: bounding the washes for the ink's sake lands
//! every one above the bound on it, and a four-colour record paints as two. The neutral chrome
//! tier carries legibility instead ([`crate::ui::backdrop::theme_backdrop`]).
//!
//! The set is fixed-size, `Brush::interpolate` blending gradients only at a matching stop count,
//! so a short answer from the quantizer is filled ([`tints`]).

use slint::Color;

use crate::ui::appearance::rgb::color_with_alpha;
use crate::ui::backdrop::{SEED_COUNT, ThemeTokens};
use melodia_artwork::media::image::material_you::{clamp_to_tone_band, rotate_hue, scale_tone};

mod dither;

pub use dither::dither_tile;

/// Washes the paint lays down, against the [`SEED_COUNT`] the quantizer is asked for: median cut
/// splits to a target, so the two can't be one number.
pub(crate) const WASH_COUNT: usize = 3;

const _: () = assert!(
    WASH_COUNT <= SEED_COUNT,
    "the paint wants more washes than the quantizer is asked for, so one would be a default"
);

/// Hue rotations for a wash with no seed, off the first colour that has one so the set can't walk
/// away from the album. Entry 0 is unreachable.
const FILL_HUES: [f64; WASH_COUNT] = [0.0, 25.0, -25.0];

/// A synthesized wash's weight. A backdrop may not invent variation: at full strength a one-hue
/// sleeve's fills read as a lightness gradient it never had.
const FILL_WEIGHT: f32 = 0.3;

/// Tone ceiling on the accent standing in for a coverless entry. An accent is picked as ink on the
/// app's surface, and at its own tone a wash of it reads as a lamp rather than ground.
const WASH_MAX_TONE: f64 = 52.0;

/// The second seated wash's tone against the first; one colour alone reads as a flat tint.
const WASH_SHADE_FACTOR: f64 = 0.8;

/// One wash's colour and how strongly it is laid on.
pub(crate) struct Tint {
    /// `0x00RRGGBB`, exactly as the quantizer answered.
    pub rgb: u32,
    /// Multiplies the falloff the Slint side paints, rather than replacing it.
    pub weight: f32,
}

impl Tint {
    /// Weight rides in the alpha, so how strongly a wash lies stays apart from its shape.
    pub(crate) fn to_color(&self) -> Color {
        color_with_alpha(self.rgb, self.weight)
    }
}

/// The washes, in the order the quantizer ranked them, untouched.
///
/// The accent never pads a short list, one hue's worth of cover getting its own colours. It stands
/// in only for an empty one, as two seeds under [`WASH_MAX_TONE`], three faint fills reading as an
/// unpainted surface. Genre Detail hands over the same shape, so the two art-less heroes agree.
pub(crate) fn tints(seeds: [Option<u32>; SEED_COUNT], theme: &ThemeTokens) -> [Tint; WASH_COUNT] {
    let mut seeds = seeds;
    // Ahead of the origin read, so the third wash fans off the seated colour rather than off the
    // pastel the other two just came down from.
    if seeds.iter().all(Option::is_none) {
        let lit = clamp_to_tone_band(theme.accent, 0.0, WASH_MAX_TONE);
        seeds[0] = Some(lit);
        seeds[1] = Some(scale_tone(lit, WASH_SHADE_FACTOR));
    }
    let origin = seeds.iter().flatten().next().copied().unwrap_or(theme.accent);

    std::array::from_fn(|wash| match seeds[wash] {
        Some(rgb) => Tint { rgb, weight: 1.0 },
        None => Tint { rgb: rotate_hue(origin, FILL_HUES[wash]), weight: FILL_WEIGHT },
    })
}

/// The washes while nothing has published: the base three times, so the band reads flat. Not
/// [`tints`]' art-less answer, which seats the accent.
pub(crate) fn idle_tints(theme: &ThemeTokens) -> [Tint; WASH_COUNT] {
    std::array::from_fn(|_| Tint { rgb: theme.base, weight: 1.0 })
}

#[cfg(test)]
#[path = "tests/aurora_tests.rs"]
mod tests;
