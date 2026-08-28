//! The tile a station with no logo paints.
//!
//! A third of a directory page carries no logo field, so this runs on roughly every third card
//! Browse draws. It is also pure and deterministic, which is the whole reason it can be tested at
//! all where the surfaces around it cannot.

use super::{StationTile, station_tile};

fn monogram_of(name: &str) -> String {
    station_tile(name).monogram.to_string()
}

/// **Digits are part of a station's identity and punctuation is not**, which is the one judgement
/// this function makes. Two stations on the same network differ by their frequency and nothing
/// else, so a monogram that dropped the number would draw them identically; a hyphen carried out
/// of a name reads as a rendering fault instead of as a name.
#[test]
fn a_monogram_keeps_digits_and_drops_punctuation() {
    assert_eq!(monogram_of("WDR 2"), "W2");
    assert_eq!(monogram_of("WDR 5"), "W5", "the number is what tells it from its sibling");
    assert_eq!(monogram_of("N-JOY"), "NJ", "the hyphen is not a word boundary and not a letter");
    assert_eq!(monogram_of("1LIVE"), "1L");
}

/// One initial per word where there are words, the first two characters where there is one — a
/// lone letter over a large tile reads as a placeholder rather than as a station.
#[test]
fn a_single_word_takes_two_letters_and_a_phrase_takes_two_initials() {
    assert_eq!(monogram_of("Deutschlandfunk"), "DE");
    assert_eq!(monogram_of("Radio Bollerwagen"), "RB");
    assert_eq!(
        monogram_of("BBC Radio 6 Music"),
        "BR",
        "two letters is the cap however many words the name has"
    );
}

/// Lowercase and non-Latin names both have to come back as something drawable — the tile paints
/// one ink over its own stops and never solves per station.
#[test]
fn a_monogram_is_uppercased_whatever_the_name_arrived_as() {
    assert_eq!(monogram_of("radio paradise"), "RP");
    assert_eq!(monogram_of("özgür radyo"), "ÖR");
}

/// **Empty is a real answer, and it is what hands the card back to the Material Symbols glyph.**
/// A name with nothing alphanumeric in it has no initials to take, and two blank letters over the
/// stops would read as a broken tile where the glyph reads as a station with no logo.
#[test]
fn a_name_with_nothing_alphanumeric_takes_no_letters() {
    assert!(monogram_of("!!!").is_empty());
    assert!(monogram_of("   ").is_empty());
    assert!(monogram_of("").is_empty());
    assert!(monogram_of("—·—").is_empty());
}

/// The tile is a hash of the name and nothing else, so a station keeps its colours across runs and
/// across the three surfaces that paint it. Were it seeded per process or per page, the same
/// station would be a different colour in Browse and in Favorites.
#[test]
fn the_same_name_always_hashes_to_the_same_tile() {
    let StationTile {
        color_1,
        color_2,
        monogram,
    } = station_tile("Radio Paradise");
    for _ in 0..4 {
        let again = station_tile("Radio Paradise");
        assert_eq!(again.color_1, color_1);
        assert_eq!(again.color_2, color_2);
        assert_eq!(again.monogram, monogram);
    }
}

/// The two stops are a gradient, so they have to differ — and they differ by hue alone at the same
/// jitter, which is what keeps one ink readable over both without being solved per station.
#[test]
fn the_two_stops_are_never_the_same_colour() {
    for name in [
        "WDR 2",
        "Radio Paradise",
        "Deutschlandfunk",
        "!!!",
        "özgür radyo",
        "1LIVE",
    ] {
        let tile = station_tile(name);
        assert_ne!(tile.color_1, tile.color_2, "{name} paints a flat tile rather than a gradient");
    }
}

/// Different stations get different tiles — the point of hashing the name rather than handing
/// every logo-less station one house colour, which is the look this replaced.
#[test]
fn different_names_land_on_different_tiles() {
    let names = [
        "WDR 2",
        "WDR 5",
        "Radio Paradise",
        "Deutschlandfunk",
        "N-JOY",
        "1LIVE",
    ];
    let mut seen: Vec<(u8, u8, u8)> = Vec::with_capacity(names.len());
    for name in names {
        let c = station_tile(name).color_1;
        seen.push((c.red(), c.green(), c.blue()));
    }
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(seen.len(), before, "two of these stations paint the same first stop");
}
