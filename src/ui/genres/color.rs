//! Deterministic per-genre tint colours.
//!
//! Genres have no intrinsic artwork, so each card / detail header gets a
//! hash-derived gradient. The derivation is [`crate::ui::name_palette`]'s,
//! shared with the station tile; this file is only the stops built on top.
//!
//! The *gradient shape* deliberately diverges from the Tauri original, which
//! used one hue plus a fixed +30° offset at constant saturation and value —
//! under that scheme hue 120° and hue 140° both just read as "green-ish".
//! Three independent name-derived axes separate them instead:
//!
//!   1. **Hue 1** — the primary, 0..360°.
//!   2. **Hue offset** — 60..120°, so a wide-offset card carries two clearly
//!      different colours while a narrow-offset one stays analogous.
//!   3. **Saturation + value jitter** — one-sided positive, so the base
//!      values act as a floor and jitter only lifts a few cards toward vivid
//!      without breaking the family look.
//!
//! Colours resolve to `slint::Color`s on `GenreRow`; the Slint side just
//! plugs them into `@linear-gradient(135deg, …)` and holds no HSV math.

use slint::Color;

use crate::ui::name_palette::{hash_name, hsv_color, hue_from_hash, hue_to_f32, jitter_0_to_15};

/// The four gradient stops a genre carries — two for the small tile
/// (grid card + detail header artwork tile, both saturated) and two
/// for the full-bleed detail hero floor (lower saturation / dimmer so
/// scrim + foreground text stay legible).
// `slint::Color` doesn't implement `Eq` (it stores ARGB as `u8`s but the
// upstream type only derives `PartialEq` + `Hash`), so we can't derive
// `Eq` on the wrapper either.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GenreAccent {
    pub tile_color_1: Color,
    pub tile_color_2: Color,
    pub hero_color_1: Color,
    pub hero_color_2: Color,
}

/// Hash the name into a `GenreAccent`. Deterministic — same name in,
/// same colours out, across runs and across the Tauri build (hash) /
/// this build (composition).
pub fn genre_accent(name: &str) -> GenreAccent {
    let hash = hash_name(name);
    let hue1 = hue_from_hash(hash);
    let offset_deg = 60 + u16::try_from(hash.rotate_right(11) % 61).unwrap_or(0);
    let hue2 = (hue1 + offset_deg) % 360;
    // Small enough to preserve the family look across the grid, large enough
    // to lift a few cards out of the monotone middle.
    let sat_jitter = jitter_0_to_15(hash.rotate_right(19));
    let val_jitter = jitter_0_to_15(hash.rotate_right(23));

    let hue1_f = hue_to_f32(hue1);
    let hue2_f = hue_to_f32(hue2);

    GenreAccent {
        // Saturated stops for the small tile.
        tile_color_1: hsv_color(hue1_f, 0.72 + sat_jitter, 0.68 + val_jitter),
        tile_color_2: hsv_color(hue2_f, 0.65 + sat_jitter, 0.48 + val_jitter),
        // Dimmer / less saturated stops for the wide hero floor, so
        // the scrim + foreground text on the detail header stays
        // readable.
        hero_color_1: hsv_color(hue1_f, 0.52 + sat_jitter, 0.52 + val_jitter),
        hero_color_2: hsv_color(hue2_f, 0.44 + sat_jitter, 0.22 + val_jitter),
    }
}

#[cfg(test)]
#[path = "tests/color_tests.rs"]
mod tests;
