//! Catppuccin theme: 4 variants (Latte / Frappe / Macchiato / Mocha) and
//! 14 accent colours, each defined per-variant. Ported verbatim from
//! `Melodia-tauri/src/themes/catppuccin.ts`.

// Palette data files are dense by design — every literal is a six-digit
// `0xRRGGBB` hex colour. `unreadable_literal` fires on anything ≥ 5
// digits, which would force underscores into every entry and obscure the
// match against the upstream CSS source.
#![allow(clippy::unreadable_literal)]

use super::{AccentDef, Palette, ThemeDef, Variant};

const LATTE: Palette = Palette {
    base: 0xeff1f5,
    mantle: 0xe6e9ef,
    crust: 0xdce0e8,
    surface0: 0xccd0da,
    surface1: 0xbcc0cc,
    surface2: 0xacb0be,
    overlay0: 0x9ca0b0,
    overlay1: 0x8c8fa1,
    overlay2: 0x7c7f93,
    text: 0x4c4f69,
    subtext0: 0x6c6f85,
    subtext1: 0x5c5f77,
    border: 0xccd0da,
    red: 0xd20f39,
    green: 0x40a02b,
    yellow: 0xdf8e1d,
};

const FRAPPE: Palette = Palette {
    base: 0x303446,
    mantle: 0x292c3c,
    crust: 0x232634,
    surface0: 0x414559,
    surface1: 0x51576d,
    surface2: 0x626880,
    overlay0: 0x737994,
    overlay1: 0x838ba7,
    overlay2: 0x949cbb,
    text: 0xc6d0f5,
    subtext0: 0xa5adce,
    subtext1: 0xb5bfe2,
    border: 0x414559,
    red: 0xe78284,
    green: 0xa6d189,
    yellow: 0xe5c890,
};

const MACCHIATO: Palette = Palette {
    base: 0x24273a,
    mantle: 0x1e2030,
    crust: 0x181926,
    surface0: 0x363a4f,
    surface1: 0x494d64,
    surface2: 0x5b6078,
    overlay0: 0x6e738d,
    overlay1: 0x8087a2,
    overlay2: 0x939ab7,
    text: 0xcad3f5,
    subtext0: 0xa5adcb,
    subtext1: 0xb8c0e0,
    border: 0x363a4f,
    red: 0xed8796,
    green: 0xa6da95,
    yellow: 0xeed49f,
};

const MOCHA: Palette = Palette {
    base: 0x1e1e2e,
    mantle: 0x181825,
    crust: 0x11111b,
    surface0: 0x313244,
    surface1: 0x45475a,
    surface2: 0x585b70,
    overlay0: 0x6c7086,
    overlay1: 0x7f849c,
    overlay2: 0x9399b2,
    text: 0xcdd6f4,
    subtext0: 0xa6adc8,
    subtext1: 0xbac2de,
    border: 0x313244,
    red: 0xf38ba8,
    green: 0xa6e3a1,
    yellow: 0xf9e2af,
};

const VARIANTS: &[Variant] = &[
    Variant {
        id: "latte",
        name: "Latte",
        palette: LATTE,
    },
    Variant {
        id: "frappe",
        name: "Frappe",
        palette: FRAPPE,
    },
    Variant {
        id: "macchiato",
        name: "Macchiato",
        palette: MACCHIATO,
    },
    Variant {
        id: "mocha",
        name: "Mocha",
        palette: MOCHA,
    },
];

const ACCENTS: &[AccentDef] = &[
    AccentDef {
        id: "rosewater",
        name: "Rosewater",
        per_variant: &[
            ("latte", 0xdc8a78),
            ("frappe", 0xf2d5cf),
            ("macchiato", 0xf4dbd6),
            ("mocha", 0xf5e0dc),
        ],
    },
    AccentDef {
        id: "flamingo",
        name: "Flamingo",
        per_variant: &[
            ("latte", 0xdd7878),
            ("frappe", 0xeebebe),
            ("macchiato", 0xf0c6c6),
            ("mocha", 0xf2cdcd),
        ],
    },
    AccentDef {
        id: "pink",
        name: "Pink",
        per_variant: &[
            ("latte", 0xea76cb),
            ("frappe", 0xf4b8e4),
            ("macchiato", 0xf5bde6),
            ("mocha", 0xf5c2e7),
        ],
    },
    AccentDef {
        id: "mauve",
        name: "Mauve",
        per_variant: &[
            ("latte", 0x8839ef),
            ("frappe", 0xca9ee6),
            ("macchiato", 0xc6a0f6),
            ("mocha", 0xcba6f7),
        ],
    },
    AccentDef {
        id: "red",
        name: "Red",
        per_variant: &[
            ("latte", 0xd20f39),
            ("frappe", 0xe78284),
            ("macchiato", 0xed8796),
            ("mocha", 0xf38ba8),
        ],
    },
    AccentDef {
        id: "maroon",
        name: "Maroon",
        per_variant: &[
            ("latte", 0xe64553),
            ("frappe", 0xea999c),
            ("macchiato", 0xee99a0),
            ("mocha", 0xeba0ac),
        ],
    },
    AccentDef {
        id: "peach",
        name: "Peach",
        per_variant: &[
            ("latte", 0xfe640b),
            ("frappe", 0xef9f76),
            ("macchiato", 0xf5a97f),
            ("mocha", 0xfab387),
        ],
    },
    AccentDef {
        id: "yellow",
        name: "Yellow",
        per_variant: &[
            ("latte", 0xdf8e1d),
            ("frappe", 0xe5c890),
            ("macchiato", 0xeed49f),
            ("mocha", 0xf9e2af),
        ],
    },
    AccentDef {
        id: "green",
        name: "Green",
        per_variant: &[
            ("latte", 0x40a02b),
            ("frappe", 0xa6d189),
            ("macchiato", 0xa6da95),
            ("mocha", 0xa6e3a1),
        ],
    },
    AccentDef {
        id: "teal",
        name: "Teal",
        per_variant: &[
            ("latte", 0x179299),
            ("frappe", 0x81c8be),
            ("macchiato", 0x8bd5ca),
            ("mocha", 0x94e2d5),
        ],
    },
    AccentDef {
        id: "sky",
        name: "Sky",
        per_variant: &[
            ("latte", 0x04a5e5),
            ("frappe", 0x99d1db),
            ("macchiato", 0x91d7e3),
            ("mocha", 0x89dceb),
        ],
    },
    AccentDef {
        id: "sapphire",
        name: "Sapphire",
        per_variant: &[
            ("latte", 0x209fb5),
            ("frappe", 0x85c1dc),
            ("macchiato", 0x7dc4e4),
            ("mocha", 0x74c7ec),
        ],
    },
    AccentDef {
        id: "blue",
        name: "Blue",
        per_variant: &[
            ("latte", 0x1e66f5),
            ("frappe", 0x8caaee),
            ("macchiato", 0x8aadf4),
            ("mocha", 0x89b4fa),
        ],
    },
    AccentDef {
        id: "lavender",
        name: "Lavender",
        per_variant: &[
            ("latte", 0x7287fd),
            ("frappe", 0xbabbf1),
            ("macchiato", 0xb7bdf8),
            ("mocha", 0xb4befe),
        ],
    },
];

pub static CATPPUCCIN: ThemeDef = ThemeDef {
    id: "catppuccin",
    name: "Catppuccin",
    variants: VARIANTS,
    accents: ACCENTS,
    default_variant: "mocha",
    default_accent: "mauve",
    supports_system_mode: true,
    system_dark_variant: "mocha",
    system_light_variant: "latte",
};
