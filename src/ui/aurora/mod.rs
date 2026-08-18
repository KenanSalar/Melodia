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
//! door: the accent stands in, seated to a wash tone and paired with a deeper sibling.
//!
//! [`dither_tile`] is the grain laid over the result and answers to the renderer rather than to
//! any artwork, so it shares nothing with the solve above but the surface it lands on.

use slint::Color;

use crate::services::material_you::{clamp_to_tone_band, rotate_hue, scale_tone};
use crate::themes::color_with_alpha;
use crate::ui::backdrop::{SEED_COUNT, ThemeTokens};

mod dither;

pub use dither::dither_tile;

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

/// Tone ceiling on the accent when it stands in for an entry with no colours of its own.
///
/// **A ceiling rather than a tone**, so an accent already this deep passes through carrying its own
/// chroma. An accent is picked to be legible as *ink on the app's surface*, which puts it well above
/// anything a record quantizes to — laid on as a wash at full tone it reads as a lamp rather than as
/// a surface, and beside Genre Detail's hashed stops the two heroes stop looking like one app. This
/// is where the accent stops being ink and becomes ground.
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
/// **Nothing is done to them** — median cut's own answer, in median cut's own order, at the alpha
/// the paint gives it. The tone band and the hue seating that used to sit here each made the
/// surface safe for the theme's ink at the cost of the thing the backdrop is for: the band merged
/// every wash above it onto one tone, and the seating spent the ranking on hue adjacency the
/// sweeps' geometry no longer needs, each owning an edge rather than a corner.
///
/// `theme`'s accent never *pads* a short list: one hue's worth of cover gets a full set of its own
/// colours, not the app's. It stands in only for an *empty* one — an entry with no artwork — and it
/// stands in as **two seeds**, seated under [`WASH_MAX_TONE`] and [`WASH_SHADE_FACTOR`] apart. Three
/// fills with nothing at full weight behind them wash at [`FILL_WEIGHT`] throughout and read as an
/// unpainted surface; one seed alone reads as a flat tint. A pair plus a fill is exactly the shape
/// Genre Detail hands over in [`crate::ui::hero_backdrop::apply_gradient`], the other hero with no
/// cover to quantize — so the two answer alike, which they visibly did not while the accent went in
/// at its own tone.
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

/// The washes a band paints while nothing has published one — the theme's base three times over,
/// so each sweep composites onto the surface it sits on and the band reads as flat `Theme.base`.
///
/// **Deliberately not [`tints`]'s art-less answer**, which seats the accent: that is what a hero
/// with nothing to quantize *is*, where this is what one that has not been asked yet is. Painting
/// the first while a collage composes is what flashed the accent across the two curated banners.
pub(crate) fn idle_tints(theme: &ThemeTokens) -> [Tint; WASH_COUNT] {
    std::array::from_fn(|_| Tint {
        rgb: theme.base,
        weight: 1.0,
    })
}

#[cfg(test)]
#[path = "tests/aurora_tests.rs"]
mod tests;
