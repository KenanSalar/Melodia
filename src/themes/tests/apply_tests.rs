//! Tests for the `kdeglobals` → `Palette` hop. The rest of `apply.rs` is either
//! Slint-facing (`apply` and `write_palette` want a live `AppWindow`) or a thin
//! `u32` ↔ `Color` conversion; `accent_brushes` and `on_accent_hex` are covered
//! by `tests/registry_tests.rs`, which walks the static tables.

use std::collections::HashMap;

use super::*;
use crate::services::system_theme::KdeColorPalette;

/// Breeze's own status foregrounds, i.e. what the mapper must land on when a
/// scheme's colour is unusable. Same values as the static `themes::kde`
/// variants, so the fallback path and the Dark/Light tables agree.
const BREEZE_RED: u32 = 0x00da_4453;
const BREEZE_GREEN: u32 = 0x0027_ae60;
const BREEZE_YELLOW: u32 = 0x00f6_7400;

/// A parsed scheme carrying every structure slot the mapper reads. The
/// structure ramp is deliberately monochrome and the semantic trio
/// deliberately not, so a crossed wire surfaces as a grey rather than as a
/// coincidental pass.
fn kde_fixture(red: &str, green: &str, yellow: &str) -> KdeColorPalette {
    let colors = [
        ("base", "#101010"),
        ("mantle", "#202020"),
        ("crust", "#303030"),
        ("surface0", "#404040"),
        ("surface1", "#505050"),
        ("surface2", "#606060"),
        ("overlay0", "#707070"),
        ("overlay1", "#808080"),
        ("overlay2", "#909090"),
        ("text", "#a0a0a0"),
        ("subtext0", "#b0b0b0"),
        ("subtext1", "#c0c0c0"),
        ("border", "#d0d0d0"),
    ]
    .into_iter()
    .map(|(key, hex)| (key.to_owned(), hex.to_owned()))
    .collect::<HashMap<_, _>>();

    KdeColorPalette {
        colors,
        accent: "#74c7ec".to_owned(),
        red: red.to_owned(),
        green: green.to_owned(),
        yellow: yellow.to_owned(),
    }
}

#[test]
fn palette_from_kde_takes_its_semantics_from_the_status_foregrounds() {
    // The regression guard for the grey traffic lights on Plasma: this mapper
    // used to fill `green` / `yellow` from `Palette::fallback_semantics`, so
    // they arrived as `overlay1` no matter what the user's colour scheme said.
    let p = palette_from_kde(&kde_fixture("#f38ba8", "#a6e3a1", "#f9e2af"));

    assert_eq!(p.red, 0x00f3_8ba8, "red ← ForegroundNegative");
    assert_eq!(p.green, 0x00a6_e3a1, "green ← ForegroundPositive");
    assert_eq!(p.yellow, 0x00f9_e2af, "yellow ← ForegroundNeutral");

    // The structure ramp still comes from the map, unchanged.
    assert_eq!(p.base, 0x0010_1010);
    assert_eq!(p.overlay1, 0x0080_8080);

    // The whole point: none of the three collapsed onto a neutral.
    for (name, semantic) in [("red", p.red), ("green", p.green), ("yellow", p.yellow)] {
        for neutral in [p.overlay0, p.overlay1, p.overlay2] {
            assert_ne!(semantic, neutral, "{name} landed on the surface ramp");
        }
    }
}

#[test]
fn palette_from_kde_semantics_fall_back_to_breeze_not_black() {
    // A structure slot the map is missing degrades to 0x000000 by design —
    // the semantic trio must not, because black reads as a deliberate colour
    // on a signal surface where grey at least reads as "off".
    let p = palette_from_kde(&kde_fixture("nonsense", "", "#gg0000"));

    assert_eq!(p.red, BREEZE_RED, "red ← Breeze ForegroundNegative default");
    assert_eq!(p.green, BREEZE_GREEN, "green ← Breeze ForegroundPositive default");
    assert_eq!(p.yellow, BREEZE_YELLOW, "yellow ← Breeze ForegroundNeutral default");
}
