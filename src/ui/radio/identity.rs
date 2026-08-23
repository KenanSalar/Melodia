//! What a station card paints when the station has no logo.
//!
//! **A third of a directory page has no logo field at all**, and holding a generic glyph up for
//! all of them is what makes Browse read as one repeated icon rather than as a list of stations.
//! So a station with nothing to draw gets something of its own instead: two stops off its name
//! and its initials over them.
//!
//! The derivation is [`crate::ui::name_palette`]'s, shared with the genre tile. What is different
//! is the range. A genre card is a colour tile and goes vivid; this one carries lettering, so the
//! stops stay deep enough that one light ink reads on every station without being solved per
//! station — `ui::backdrop`'s aurora arm makes the same trade for the same reason.

use slint::{Color, SharedString};

use crate::ui::name_palette::{hash_name, hsv_color, hue_from_hash, hue_to_f32, jitter_0_to_15};

/// How far the second stop's hue sits from the first. Narrow, unlike the genre tile's 60..120°:
/// two clearly different colours behind lettering read as a graphic rather than as a ground.
const HUE_OFFSET_MIN: u16 = 20;
const HUE_OFFSET_SPAN: u32 = 26;

/// Saturation and value the stops are built from. Both stops sit well under the midpoint so the
/// ink over them never has to be solved; the jitter only lifts, so these are floors.
const STOP_1_SATURATION: f32 = 0.44;
const STOP_1_VALUE: f32 = 0.34;
const STOP_2_SATURATION: f32 = 0.52;
const STOP_2_VALUE: f32 = 0.20;

/// Letters a monogram shows. Two, because one is not enough to tell two stations apart and three
/// stops fitting the tile at the narrowest column count.
const MONOGRAM_LEN: usize = 2;

/// The tile a logo-less station paints: two stops and the letters over them.
pub struct StationTile {
    pub color_1: Color,
    pub color_2: Color,
    pub monogram: SharedString,
}

/// Hash `name` into a station's own tile. Deterministic, so a station keeps its colours across
/// runs and across the page it is found on.
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

/// A station's initials.
///
/// One initial per word where the name has words, so "Radio Bollerwagen" reads RB; the first two
/// characters where it does not, so "Deutschlandfunk" reads DE rather than a lone D.
///
/// **Digits count and punctuation does not.** Station names are full of both, and they are not the
/// same kind of character here: the number *is* the identity for a "WDR 2" beside a "WDR 5", where
/// a hyphen carried out of "N-JOY" reads as a rendering fault rather than as a name.
///
/// Empty for a name with nothing alphanumeric in it, which is what leaves the card on the glyph.
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
        // One word, so the second character comes from inside it rather than from a word that
        // isn't there.
        None => name.chars().filter(|ch| ch.is_alphanumeric()).take(MONOGRAM_LEN).collect(),
    };
    letters.to_uppercase()
}
