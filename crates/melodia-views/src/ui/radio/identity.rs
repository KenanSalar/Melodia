//! What a station card paints when the station has no logo: two stops off its name and its
//! initials over them, so a page of logo-less stations doesn't read as one repeated icon.
//!
//! The derivation is [`crate::ui::name_palette`]'s, shared with the genre tile, but the stops stay
//! deep so one light ink reads on every station without a per-station solve.

use slint::{Color, SharedString};

use crate::ui::name_palette::{hash_name, hsv_color, hue_from_hash, hue_to_f32, jitter_0_to_15};

/// How far the second stop's hue sits from the first, narrower than the genre tile's: two clearly
/// different colours behind lettering read as a graphic rather than a ground.
const HUE_OFFSET_MIN: u16 = 20;
const HUE_OFFSET_SPAN: u32 = 26;

/// The stops' saturation and value, floors well under the midpoint (the jitter only lifts) so the
/// ink over them never has to be solved.
const STOP_1_SATURATION: f32 = 0.44;
const STOP_1_VALUE: f32 = 0.34;
const STOP_2_SATURATION: f32 = 0.52;
const STOP_2_VALUE: f32 = 0.20;

/// Letters a monogram shows: one can't tell two stations apart, and three stop fitting the tile at
/// the narrowest column count.
const MONOGRAM_LEN: usize = 2;

/// The tile a logo-less station paints: two stops and the letters over them.
pub struct StationTile {
    pub color_1: Color,
    pub color_2: Color,
    pub monogram: SharedString,
}

/// Hashes `name` into a station's own tile, so it keeps its colours across runs and pages.
pub fn station_tile(name: &str) -> StationTile {
    let hash = hash_name(name);
    let hue_1 = hue_from_hash(hash);
    let offset =
        HUE_OFFSET_MIN + u16::try_from(hash.rotate_right(11) % HUE_OFFSET_SPAN).unwrap_or(0);
    let hue_2 = (hue_1 + offset) % 360;
    let saturation_jitter = jitter_0_to_15(hash.rotate_right(19));
    let value_jitter = jitter_0_to_15(hash.rotate_right(23));

    StationTile {
        color_1: hsv_color(
            hue_to_f32(hue_1),
            STOP_1_SATURATION + saturation_jitter,
            STOP_1_VALUE + value_jitter,
        ),
        color_2: hsv_color(
            hue_to_f32(hue_2),
            STOP_2_SATURATION + saturation_jitter,
            STOP_2_VALUE + value_jitter,
        ),
        monogram: SharedString::from(monogram(name)),
    }
}

/// A station's initials: one per word ("Radio Bollerwagen" reads RB), or a lone word's first two
/// characters ("Deutschlandfunk" reads DE). Digits count, telling "WDR 2" from "WDR 5", and
/// punctuation doesn't. Empty where nothing is alphanumeric, which leaves the card on the glyph.
fn monogram(name: &str) -> String {
    let mut initials = name
        .split_whitespace()
        .filter_map(|word| word.chars().find(|ch| ch.is_alphanumeric()))
        .take(MONOGRAM_LEN);

    let Some(first) = initials.next() else {
        return String::new();
    };
    let letters: String = match initials.next() {
        Some(second) => [first, second].into_iter().collect(),
        None => name.chars().filter(|ch| ch.is_alphanumeric()).take(MONOGRAM_LEN).collect(),
    };
    letters.to_uppercase()
}

#[cfg(test)]
#[path = "tests/identity_tests.rs"]
mod tests;
