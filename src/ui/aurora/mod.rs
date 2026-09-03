//! The colours an aurora backdrop washes over its base, from the artwork's own seeds.
//!
//! **Each wash is the quantizer's answer, untouched** — no tone band, no hue seating, no
//! reordering, so a sleeve's value structure reaches the backdrop intact. Bounding it to keep the
//! ink legible is what flattens it: every wash above the bound lands *on* the bound and a
//! four-colour record paints as two. Contrast is whatever the cover gives, and the neutral chrome
//! tier ([`crate::ui::backdrop::theme_backdrop`]) carries the readable half.
//!
//! The set is fixed-size — `Brush::interpolate` blends gradients only at a matching stop count —
//! and the quantizer can still answer short, hence [`tints`]'s filling rule.
//!
//! [`dither_tile`] answers to the renderer rather than to any artwork, sharing nothing with the
//! solve above but the surface it lands on.

use slint::Color;

use crate::services::material_you::{clamp_to_tone_band, rotate_hue, scale_tone};
use crate::ui::appearance::theme_apply::color_with_alpha;
use crate::ui::backdrop::{SEED_COUNT, ThemeTokens};

mod dither;

pub use dither::dither_tile;

/// Washes the paint lays down, against the [`SEED_COUNT`] the quantizer is asked for.
///
/// **Deliberately not one number.** `aurora-backdrop.slint` mounts three sweeps; median cut splits
/// to a *target*, so asking it for three yields different boxes rather than these minus the last.
pub(crate) const WASH_COUNT: usize = 3;

const _: () = assert!(
    WASH_COUNT <= SEED_COUNT,
    "the paint wants more washes than the quantizer is asked for, so one would be a default"
);

/// Hue rotation for a wash the quantizer had no seed for, always off the first colour that does
/// exist — rotating from the previous fill lets the set walk away from the album. A fan either side
/// at the analogous step. Entry 0 is unreachable, [`tints`] never leaving the first slot empty.
const FILL_HUES: [f64; WASH_COUNT] = [0.0, 25.0, -25.0];

/// How faintly a synthesized tint is laid on, against 1.0 for one the artwork offered. **A backdrop
/// may not invent variation the record doesn't have** — the fills of a one-hue sleeve differ only
/// in direction, and at full strength a stack of those is a lightness gradient it never had.
const FILL_WEIGHT: f32 = 0.3;

/// Tone ceiling on the accent when it stands in for an entry with no colours of its own — a
/// ceiling rather than a tone, so an accent already this deep keeps its own chroma. An accent is
/// picked to be legible as *ink on the app's surface*, well above anything a record quantizes to;
/// at full tone it reads as a lamp rather than as ground.
const WASH_MAX_TONE: f64 = 52.0;

/// How far the second seated wash sits below the first. The pair is what an art-less surface has
/// instead of a cover's value structure, and one colour alone reads as a flat tint however it is
/// swept.
const WASH_SHADE_FACTOR: f64 = 0.8;

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
/// **Nothing is done to them.** The tone band and the hue seating that used to sit here each made
/// the surface safe for the theme's ink at the cost of what the backdrop is for: the band merged
/// every wash above it onto one tone, the seating spent the ranking on hue adjacency the sweeps'
/// geometry no longer needs.
///
/// `theme`'s accent never *pads* a short list — one hue's worth of cover gets its own colours, not
/// the app's. It stands in only for an *empty* one, and as **two seeds**, under [`WASH_MAX_TONE`]
/// and [`WASH_SHADE_FACTOR`] apart: three fills at [`FILL_WEIGHT`] read as an unpainted surface, one
/// seed alone as a flat tint. That pair-plus-a-fill is the shape Genre Detail hands
/// [`crate::ui::hero_backdrop::apply_gradient`], so the two art-less heroes answer alike.
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

/// The washes a band paints while nothing has published one — the theme's base three times over, so
/// the band reads as flat `Theme.base`.
///
/// **Deliberately not [`tints`]'s art-less answer**, which seats the accent: that is a hero with
/// nothing to quantize, this is one not yet asked. Painting the first while a collage composes is
/// what flashed the accent across the two curated banners.
pub(crate) fn idle_tints(theme: &ThemeTokens) -> [Tint; WASH_COUNT] {
    std::array::from_fn(|_| Tint {
        rgb: theme.base,
        weight: 1.0,
    })
}

#[cfg(test)]
#[path = "tests/aurora_tests.rs"]
mod tests;
