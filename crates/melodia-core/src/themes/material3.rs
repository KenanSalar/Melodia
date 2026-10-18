//! Material 3 theme: 2 variants (Dark / Light), 8 accents.
//!
//! The two variants are generated rather than picked: each is
//! `media::image::material_you::generate_palette` over the M3 baseline seed `#6750a4` (the Purple
//! accent's light value) in Tonal Spot, at the colour spec that module pins. So before any artwork
//! lands, Material You's Tonal Spot paints exactly this. The accents are key colours, which the
//! spec doesn't define, so they stay as picked.

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
    base: 0x0f0d12,
    mantle: 0x141218,
    crust: 0x0f0d12,
    surface0: 0x211e26,
    surface1: 0x27242d,
    surface2: 0x4f4b55,
    overlay0: 0x4a4650,
    overlay1: 0x78737e,
    overlay2: 0xaea9b4,
    text: 0xeae3ef,
    subtext0: 0xccc6d1,
    subtext1: 0x78737e,
    border: 0x27242d,
    card_control: 0x49454e,
    card_control_hover: 0x5d5962,
    red: 0xf97386,
    yellow: DARK_YELLOW,
    green: DARK_GREEN,
};

const LIGHT: Palette = Palette {
    base: 0xfdf7fe,
    mantle: 0xf8f1fa,
    crust: 0xded8e4,
    surface0: 0xece6f0,
    surface1: 0xe7e0ec,
    surface2: 0xb2acb7,
    overlay0: 0xb5b0bb,
    overlay1: 0x7d7983,
    overlay2: 0x615d68,
    text: 0x34313a,
    subtext0: 0x4a4751,
    subtext1: 0x7d7983,
    border: 0xe7e0ec,
    card_control: 0xc7c2cc,
    card_control_hover: 0xb5b0b9,
    red: 0xa8364b,
    yellow: LIGHT_YELLOW,
    green: LIGHT_GREEN,
};

const VARIANTS: &[Variant] = &[
    Variant { id: "dark", name: "Dark", palette: DARK, mantle_unfocused: None },
    Variant { id: "light", name: "Light", palette: LIGHT, mantle_unfocused: None },
];

const ACCENTS: &[AccentDef] = &[
    AccentDef {
        id: "purple",
        name: "Purple",
        per_variant: &[("dark", 0xd0bcff), ("light", 0x6750a4)],
    },
    AccentDef { id: "blue", name: "Blue", per_variant: &[("dark", 0xa8c7fa), ("light", 0x0b57d0)] },
    AccentDef { id: "teal", name: "Teal", per_variant: &[("dark", 0x81d5c8), ("light", 0x006b5c)] },
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
    AccentDef { id: "red", name: "Red", per_variant: &[("dark", 0xffb4ab), ("light", 0xb3261e)] },
    AccentDef { id: "pink", name: "Pink", per_variant: &[("dark", 0xffb1c8), ("light", 0x984061)] },
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
