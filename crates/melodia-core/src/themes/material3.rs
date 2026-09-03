//! Material 3 theme: 2 variants (Dark / Light), 8 accents. Static palette
//! only — the dynamic-from-album-art "Color Style" feature is deferred.
//! Ported verbatim from `Melodia-tauri/src/themes/material3.ts`.

#![allow(clippy::unreadable_literal)]

use super::{AccentDef, Palette, ThemeDef, Variant};

// Traffic-light green / yellow, one pair per polarity. Same hues as the Green
// and Yellow accents, so they read as traffic-light colours on an M3 palette;
// the light pair is a touch more saturated than the MD3 reference `tertiary`
// slots, which are too washed to signal anything at these tones.
//
// Exported because the Material You dynamic palette reuses them verbatim
// (`media::image::material_you::generate_palette`), from another crate. A dynamic
// scheme has no green or yellow role to map, and deriving one from the album seed
// puts a moving colour on the three surfaces that need a fixed one — maximize, the
// success/warning toasts, and the star rating.
pub const DARK_GREEN: u32 = 0xa8dab5;
pub const DARK_YELLOW: u32 = 0xe2c569;
pub const LIGHT_GREEN: u32 = 0x1b6d2f;
pub const LIGHT_YELLOW: u32 = 0xc69a17;

const DARK: Palette = Palette {
    base: 0x141218,
    mantle: 0x1d1b20,
    crust: 0x0f0d13,
    surface0: 0x2b2930,
    surface1: 0x36343b,
    surface2: 0x413f46,
    overlay0: 0x49454f,
    overlay1: 0x79747e,
    overlay2: 0x938f99,
    text: 0xe6e0e9,
    subtext0: 0xcac4d0,
    subtext1: 0x938f99,
    border: 0x36343b,
    red: 0xffb4ab,
    yellow: DARK_YELLOW,
    green: DARK_GREEN,
};

const LIGHT: Palette = Palette {
    base: 0xfef7ff,
    mantle: 0xf7f2fa,
    crust: 0xf3edf7,
    surface0: 0xece6f0,
    surface1: 0xe4dfe8,
    surface2: 0xddd8e0,
    overlay0: 0xcac4d0,
    overlay1: 0x938f99,
    overlay2: 0x79747e,
    text: 0x1d1b20,
    subtext0: 0x49454f,
    subtext1: 0x79747e,
    border: 0xcac4d0,
    red: 0xb3261e,
    yellow: LIGHT_YELLOW,
    green: LIGHT_GREEN,
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
        id: "purple",
        name: "Purple",
        per_variant: &[("dark", 0xd0bcff), ("light", 0x6750a4)],
    },
    AccentDef {
        id: "blue",
        name: "Blue",
        per_variant: &[("dark", 0xa8c7fa), ("light", 0x0b57d0)],
    },
    AccentDef {
        id: "teal",
        name: "Teal",
        per_variant: &[("dark", 0x81d5c8), ("light", 0x006b5c)],
    },
    AccentDef {
        id: "green",
        name: "Green",
        per_variant: &[("dark", 0xa8dab5), ("light", 0x1b6d2f)],
    },
    AccentDef {
        id: "yellow",
        name: "Yellow",
        per_variant: &[("dark", 0xe2c569), ("light", 0x6d5e00)],
    },
    AccentDef {
        id: "orange",
        name: "Orange",
        per_variant: &[("dark", 0xffb77c), ("light", 0x9c4500)],
    },
    AccentDef {
        id: "red",
        name: "Red",
        per_variant: &[("dark", 0xffb4ab), ("light", 0xb3261e)],
    },
    AccentDef {
        id: "pink",
        name: "Pink",
        per_variant: &[("dark", 0xffb1c8), ("light", 0x984061)],
    },
];

pub static MATERIAL3: ThemeDef = ThemeDef {
    id: "material3",
    name: "Material 3",
    variants: VARIANTS,
    accents: ACCENTS,
    default_variant: "dark",
    default_accent: "purple",
    supports_system_mode: true,
    system_dark_variant: "dark",
    system_light_variant: "light",
};
