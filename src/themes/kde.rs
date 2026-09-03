//! KDE Breeze theme: 2 variants (Dark / Light), 8 accents.
//! Ported verbatim from `Melodia-tauri/src/themes/kde-breeze.ts`.
//!
//! The static tables are half of it. The other half is [`palette_from_kde`], which re-sources
//! every slot from the live `kdeglobals` so the player matches Plasma's active scheme rather than
//! this file's snapshot of it. Both answer for the same theme, so they live together.

#![allow(clippy::unreadable_literal)]

use super::{AccentDef, Palette, ThemeDef, Variant};

const DARK: Palette = Palette {
    base: 0x232629,
    mantle: 0x272c31,
    crust: 0x161618,
    surface0: 0x31363b,
    surface1: 0x3b4045,
    surface2: 0x454b51,
    overlay0: 0x454b51,
    overlay1: 0x7f8c8d,
    overlay2: 0xa1a9b1,
    text: 0xfcfcfc,
    subtext0: 0xbdc3c7,
    subtext1: 0xa1a9b1,
    border: 0x3f444a,
    red: 0xda4453,
    // KDE Breeze positive / neutral tokens — keep the macOS-style
    // titlebar traffic lights vivid on a Breeze palette. (Yellow uses
    // the warmer `BrightOrange` shade for better contrast against the
    // mantle than Breeze's bland `BrightSun` yellow.)
    yellow: 0xf67400,
    green: 0x27ae60,
};

const LIGHT: Palette = Palette {
    base: 0xeff0f1,
    mantle: 0xe3e5e7,
    crust: 0xd3d5d8,
    surface0: 0xd3d4d6,
    surface1: 0xc7c9cb,
    surface2: 0xbbbdbf,
    overlay0: 0xc9cdd1,
    overlay1: 0x939ba3,
    overlay2: 0x707d8a,
    text: 0x232629,
    subtext0: 0x585e64,
    subtext1: 0x707d8a,
    border: 0xc9cdd1,
    red: 0xda4453,
    yellow: 0xf67400,
    green: 0x27ae60,
};

const VARIANTS: &[Variant] = &[
    Variant {
        id: "dark",
        name: "Dark",
        palette: DARK,
    },
    Variant {
        id: "light",
        name: "Light",
        palette: LIGHT,
    },
];

const ACCENTS: &[AccentDef] = &[
    AccentDef {
        id: "blue",
        name: "Blue",
        per_variant: &[("dark", 0x3daee9), ("light", 0x2980b9)],
    },
    AccentDef {
        id: "teal",
        name: "Teal",
        per_variant: &[("dark", 0x2bc4ac), ("light", 0x038387)],
    },
    AccentDef {
        id: "green",
        name: "Green",
        per_variant: &[("dark", 0x27ae60), ("light", 0x1d8348)],
    },
    AccentDef {
        id: "orange",
        name: "Orange",
        per_variant: &[("dark", 0xf67400), ("light", 0xca5010)],
    },
    AccentDef {
        id: "red",
        name: "Red",
        per_variant: &[("dark", 0xda4453), ("light", 0xc0392b)],
    },
    AccentDef {
        id: "purple",
        name: "Purple",
        per_variant: &[("dark", 0x9b59b6), ("light", 0x7d3c98)],
    },
    AccentDef {
        id: "pink",
        name: "Pink",
        per_variant: &[("dark", 0xe966a0), ("light", 0xbf0077)],
    },
    AccentDef {
        id: "slate",
        name: "Slate",
        per_variant: &[("dark", 0x7f8c8d), ("light", 0x566573)],
    },
];

pub static KDE: ThemeDef = ThemeDef {
    id: "kde-breeze",
    name: "KDE",
    variants: VARIANTS,
    accents: ACCENTS,
    default_variant: "dark",
    default_accent: "blue",
    supports_system_mode: true,
    system_dark_variant: "dark",
    system_light_variant: "light",
};

/// A `"#RRGGBB"` hex string as the packed `0x00RRGGBB` the palette tables use.
/// `None` on malformed input, which callers answer with a default.
#[cfg(target_os = "linux")]
pub fn parse_hex_color(s: &str) -> Option<u32> {
    let stripped = s.strip_prefix('#').unwrap_or(s);
    u32::from_str_radix(stripped, 16).ok()
}

/// A `Palette` from a parsed `kdeglobals` scheme: the structure slots straight
/// off the colours map, the three semantic ones from Plasma's own status
/// foregrounds.
///
/// The Breeze hexes below are a second line rather than the policy —
/// `kde_palette_from_sections` already substitutes the same defaults for a
/// scheme that omits a status foreground, and always hands back something
/// parseable. They fire only if that stops being true.
#[cfg(target_os = "linux")]
pub fn palette_from_kde(kde: &crate::services::platform::system_theme::KdeColorPalette) -> Palette {
    let g =
        |key: &str| -> u32 { kde.colors.get(key).and_then(|s| parse_hex_color(s)).unwrap_or(0) };
    let overlay1 = g("overlay1");
    Palette {
        base: g("base"),
        mantle: g("mantle"),
        crust: g("crust"),
        surface0: g("surface0"),
        surface1: g("surface1"),
        surface2: g("surface2"),
        overlay0: g("overlay0"),
        overlay1,
        overlay2: g("overlay2"),
        text: g("text"),
        subtext0: g("subtext0"),
        subtext1: g("subtext1"),
        border: g("border"),
        red: parse_hex_color(&kde.red).unwrap_or(0x00da_4453),
        green: parse_hex_color(&kde.green).unwrap_or(0x0027_ae60),
        yellow: parse_hex_color(&kde.yellow).unwrap_or(0x00f6_7400),
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "tests/kde_tests.rs"]
mod tests;
