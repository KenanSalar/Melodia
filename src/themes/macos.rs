//! macOS theme: 2 variants (Dark / Light), 8 accents.
//! Ported verbatim from `Melodia-tauri/src/themes/macos-theme.ts`.

#![allow(clippy::unreadable_literal)]

use super::{AccentDef, Palette, ThemeDef, Variant};

const DARK: Palette = Palette {
    base:     0x1e1e1e,
    mantle:   0x252525,
    crust:    0x1a1a1a,
    surface0: 0x323232,
    surface1: 0x3a3a3a,
    surface2: 0x434343,
    overlay0: 0x3d3d3d,
    overlay1: 0x545454,
    overlay2: 0x6e6e6e,
    text:     0xffffff,
    subtext0: 0xababab,
    subtext1: 0x8e8e8e,
    border:   0x3e3e3e,
    red:      0xff453a,
    // System Yellow / Green Dark mode (mirrors the macOS accents of the
    // same name) — used by the macOS-style titlebar traffic lights so
    // they pick up the active palette rather than falling back to grey.
    yellow:   0xffd60a,
    green:    0x30d158,
    ..Palette::fallback_semantics(0x545454)
};

const LIGHT: Palette = Palette {
    base:     0xf5f5f7,
    mantle:   0xe8e8ed,
    crust:    0xdcdce1,
    surface0: 0xd7d7dc,
    surface1: 0xcbcbd0,
    surface2: 0xbfbfc4,
    overlay0: 0xd2d2d7,
    overlay1: 0xaeaeb2,
    overlay2: 0x8e8e93,
    text:     0x1d1d1f,
    subtext0: 0x86868b,
    subtext1: 0x636366,
    border:   0xd2d2d7,
    red:      0xff3b30,
    // System Yellow / Green Light mode.
    yellow:   0xffcc00,
    green:    0x28cd41,
    ..Palette::fallback_semantics(0xaeaeb2)
};

const VARIANTS: &[Variant] = &[
    Variant { id: "dark",  name: "Dark",  palette: DARK  },
    Variant { id: "light", name: "Light", palette: LIGHT },
];

const ACCENTS: &[AccentDef] = &[
    AccentDef { id: "blue",     name: "Blue",     per_variant: &[("dark", 0x0a84ff), ("light", 0x007aff)] },
    AccentDef { id: "purple",   name: "Purple",   per_variant: &[("dark", 0xbf5af2), ("light", 0x5856d6)] },
    AccentDef { id: "pink",     name: "Pink",     per_variant: &[("dark", 0xff375f), ("light", 0xff2d55)] },
    AccentDef { id: "red",      name: "Red",      per_variant: &[("dark", 0xff453a), ("light", 0xff3b30)] },
    AccentDef { id: "orange",   name: "Orange",   per_variant: &[("dark", 0xff9f0a), ("light", 0xff9500)] },
    AccentDef { id: "yellow",   name: "Yellow",   per_variant: &[("dark", 0xffd60a), ("light", 0xffcc00)] },
    AccentDef { id: "green",    name: "Green",    per_variant: &[("dark", 0x30d158), ("light", 0x28cd41)] },
    AccentDef { id: "graphite", name: "Graphite", per_variant: &[("dark", 0x98989d), ("light", 0x8e8e93)] },
];

pub static MACOS: ThemeDef = ThemeDef {
    id: "macos",
    name: "macOS",
    variants: VARIANTS,
    accents: ACCENTS,
    default_variant: "dark",
    default_accent: "blue",
    supports_system_mode: true,
    system_dark_variant: "dark",
    system_light_variant: "light",
};
