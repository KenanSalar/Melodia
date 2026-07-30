//! Windows Fluent theme: 2 variants (Dark / Light), 8 accents.
//! Ported verbatim from `Melodia-tauri/src/themes/windows-fluent.ts`.

#![allow(clippy::unreadable_literal)]

use super::{AccentDef, Palette, ThemeDef, Variant};

const DARK: Palette = Palette {
    base:     0x202020,
    mantle:   0x1c1c1c,
    crust:    0x171717,
    surface0: 0x2d2d2d,
    surface1: 0x383838,
    surface2: 0x444444,
    overlay0: 0x555555,
    overlay1: 0x767676,
    overlay2: 0x8a8a8a,
    text:     0xffffff,
    subtext0: 0xc5c5c5,
    subtext1: 0xd4d4d4,
    border:   0x3d3d3d,
    red:      0xff6767,
    // Win11 Fluent positive / cautionary tokens — mirror the existing
    // Green / Gold accents so the macOS-style titlebar traffic lights
    // read correctly on a Windows theme.
    yellow:   0xffd700,
    green:    0x6ccb5f,
};

const LIGHT: Palette = Palette {
    base:     0xf3f3f3,
    mantle:   0xebebeb,
    crust:    0xe0e0e0,
    surface0: 0xd5d5d5,
    surface1: 0xc9c9c9,
    surface2: 0xbdbdbd,
    overlay0: 0xc4c4c4,
    overlay1: 0x9e9e9e,
    overlay2: 0x757575,
    text:     0x1b1b1b,
    subtext0: 0x616161,
    subtext1: 0x424242,
    border:   0xc4c4c4,
    red:      0xc42b1c,
    yellow:   0xeab308,
    green:    0x107c10,
};

const VARIANTS: &[Variant] = &[
    Variant { id: "dark",  name: "Dark",  palette: DARK  },
    Variant { id: "light", name: "Light", palette: LIGHT },
];

const ACCENTS: &[AccentDef] = &[
    AccentDef { id: "blue",   name: "Blue",   per_variant: &[("dark", 0x60cdff), ("light", 0x005fb8)] },
    AccentDef { id: "purple", name: "Purple", per_variant: &[("dark", 0xb4a0ff), ("light", 0x7160e8)] },
    AccentDef { id: "teal",   name: "Teal",   per_variant: &[("dark", 0x4cd7d0), ("light", 0x038387)] },
    AccentDef { id: "green",  name: "Green",  per_variant: &[("dark", 0x6ccb5f), ("light", 0x107c10)] },
    AccentDef { id: "red",    name: "Red",    per_variant: &[("dark", 0xff6767), ("light", 0xc42b1c)] },
    AccentDef { id: "orange", name: "Orange", per_variant: &[("dark", 0xffa24b), ("light", 0xca5010)] },
    AccentDef { id: "pink",   name: "Pink",   per_variant: &[("dark", 0xff7eb3), ("light", 0xbf0077)] },
    AccentDef { id: "gold",   name: "Gold",   per_variant: &[("dark", 0xffd700), ("light", 0x986f0b)] },
];

pub static WINDOWS: ThemeDef = ThemeDef {
    id: "windows-fluent",
    name: "Windows",
    variants: VARIANTS,
    accents: ACCENTS,
    default_variant: "dark",
    default_accent: "blue",
    supports_system_mode: true,
    system_dark_variant: "dark",
    system_light_variant: "light",
};
