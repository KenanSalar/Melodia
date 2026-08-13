//! KDE Breeze theme: 2 variants (Dark / Light), 8 accents.
//! Ported verbatim from `Melodia-tauri/src/themes/kde-breeze.ts`.

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
