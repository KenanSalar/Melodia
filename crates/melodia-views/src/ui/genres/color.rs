//! The gradient a genre paints in place of artwork, hashed off its name through
//! [`crate::ui::name_palette`]'s derivation, which the station tile shares.
//!
//! Three name-derived axes keep neighbouring hues apart, where one hue plus a fixed offset reads
//! 120° and 140° both as green: the primary hue; a 60–120° offset to the second stop, so a wide
//! one carries two colours and a narrow one stays analogous; and a one-sided saturation and value
//! jitter that lifts a few cards toward vivid without breaking the family look.

use slint::Color;

use crate::ui::name_palette::{hash_name, hsv_color, hue_from_hash, hue_to_f32, jitter_0_to_15};

/// A genre's four stops: a saturated pair for the card and the detail's square, and a dimmer one
/// for the hero floor, so the scrim and the text over it stay legible.
// No `Eq`: `slint::Color` derives only `PartialEq`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GenreAccent {
    pub tile_color_1: Color,
    pub tile_color_2: Color,
    pub hero_color_1: Color,
    pub hero_color_2: Color,
}

/// Hashes the name into a `GenreAccent`, the same colours across runs and releases.
pub fn genre_accent(name: &str) -> GenreAccent {
    let hash = hash_name(name);
    let hue1 = hue_from_hash(hash);
    let offset_deg = 60 + u16::try_from(hash.rotate_right(11) % 61).unwrap_or(0);
    let hue2 = (hue1 + offset_deg) % 360;
    let sat_jitter = jitter_0_to_15(hash.rotate_right(19));
    let val_jitter = jitter_0_to_15(hash.rotate_right(23));

    let hue1_f = hue_to_f32(hue1);
    let hue2_f = hue_to_f32(hue2);

    GenreAccent {
        tile_color_1: hsv_color(hue1_f, 0.72 + sat_jitter, 0.68 + val_jitter),
        tile_color_2: hsv_color(hue2_f, 0.65 + sat_jitter, 0.48 + val_jitter),
        hero_color_1: hsv_color(hue1_f, 0.52 + sat_jitter, 0.52 + val_jitter),
        hero_color_2: hsv_color(hue2_f, 0.44 + sat_jitter, 0.22 + val_jitter),
    }
}

#[cfg(test)]
#[path = "tests/color_tests.rs"]
mod tests;
