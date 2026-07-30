//! GNOME Adwaita theme: 2 variants (Dark / Light), 9 accents.
//! Ported verbatim from `Melodia-tauri/src/themes/gnome-adwaita.ts`.

#![allow(clippy::unreadable_literal)]

use super::{AccentDef, Palette, ThemeDef, Variant};

const DARK: Palette = Palette {
    base:     0x222226,
    mantle:   0x2e2e32,
    crust:    0x1d1d20,
    surface0: 0x383840,
    surface1: 0x43434a,
    surface2: 0x4a4a52,
    overlay0: 0x4a4a52,
    overlay1: 0x808088,
    overlay2: 0xa0a0a8,
    text:     0xffffff,
    subtext0: 0xb0b0b8,
    subtext1: 0x8c8c94,
    border:   0x434349,
    red:      0xc01c28,
    // GNOME Adwaita success / warning tokens (GNOME 4) — keep the
    // macOS-style titlebar traffic lights in palette on GNOME themes.
    yellow:   0xf6d32d,
    green:    0x57e389,
};

const LIGHT: Palette = Palette {
    base:     0xfafafb,
    mantle:   0xebebed,
    crust:    0xe0e0e4,
    surface0: 0xdcdcdf,
    surface1: 0xd0d0d4,
    surface2: 0xc4c4c8,
    overlay0: 0xd0d0d4,
    overlay1: 0x929296,
    overlay2: 0x6c6c70,
    text:     0x1e1e24,
    subtext0: 0x737378,
    subtext1: 0x505054,
    border:   0xd0d0d4,
    red:      0xe01b24,
    yellow:   0xe5a50a,
    green:    0x33d17a,
};

const VARIANTS: &[Variant] = &[
    Variant { id: "dark",  name: "Dark",  palette: DARK  },
    Variant { id: "light", name: "Light", palette: LIGHT },
];

const ACCENTS: &[AccentDef] = &[
    AccentDef { id: "blue",   name: "Blue",   per_variant: &[("dark", 0x81d0ff), ("light", 0x0461be)] },
    AccentDef { id: "teal",   name: "Teal",   per_variant: &[("dark", 0x7bdff4), ("light", 0x007184)] },
    AccentDef { id: "green",  name: "Green",  per_variant: &[("dark", 0x8de698), ("light", 0x15772e)] },
    AccentDef { id: "yellow", name: "Yellow", per_variant: &[("dark", 0xffc057), ("light", 0x905300)] },
    AccentDef { id: "orange", name: "Orange", per_variant: &[("dark", 0xff9c5b), ("light", 0xb62200)] },
    AccentDef { id: "red",    name: "Red",    per_variant: &[("dark", 0xff888c), ("light", 0xc00023)] },
    AccentDef { id: "pink",   name: "Pink",   per_variant: &[("dark", 0xffa0d8), ("light", 0xa2326c)] },
    AccentDef { id: "purple", name: "Purple", per_variant: &[("dark", 0xfba7ff), ("light", 0x8939a4)] },
    AccentDef { id: "slate",  name: "Slate",  per_variant: &[("dark", 0xbbd1e5), ("light", 0x526678)] },
];

pub static GNOME: ThemeDef = ThemeDef {
    id: "gnome-adwaita",
    name: "GNOME",
    variants: VARIANTS,
    accents: ACCENTS,
    default_variant: "dark",
    default_accent: "blue",
    supports_system_mode: true,
    system_dark_variant: "dark",
    system_light_variant: "light",
};
