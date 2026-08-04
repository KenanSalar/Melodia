//! Pure data structures for the theme registry. No Slint imports, no OS
//! signals — those live in `apply.rs` and `system_color_state.rs`
//! respectively.

/// The theme-dependent brush slots that come from a palette table. Stored as
/// packed `0x00RRGGBB` so the data tables stay readable next to the
/// Tauri-source hex strings. `apply()` writes three more that don't:
/// `mantle_unfocused` (an OS signal), and `accent` / `accent_text` (picked
/// independently of the variant).
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    // 13 base / structure
    pub base: u32,
    pub mantle: u32,
    pub crust: u32,
    pub surface0: u32,
    pub surface1: u32,
    pub surface2: u32,
    pub overlay0: u32,
    pub overlay1: u32,
    pub overlay2: u32,
    pub text: u32,
    pub subtext0: u32,
    pub subtext1: u32,
    pub border: u32,
    // 3 semantic. Every palette names all three — there is deliberately no
    // struct-update fallback to fill them, because the one that used to exist
    // let the two generated palettes (Material You, KDE-from-kdeglobals) ship
    // a neutral grey into `green` / `yellow` without anyone noticing. The
    // surfaces reading them are signals: the macOS-style traffic lights, the
    // success/warning toasts, the star rating.
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Variant {
    pub id: &'static str,
    pub name: &'static str,
    pub palette: Palette,
}

#[derive(Clone, Copy, Debug)]
pub struct AccentDef {
    pub id: &'static str,
    pub name: &'static str,
    /// `(variant-id, 0xRRGGBB)` pairs. Linear scan — at most 4 entries.
    pub per_variant: &'static [(&'static str, u32)],
}

impl AccentDef {
    /// Hex for this accent in the given variant; falls back to the first
    /// listed variant if the requested one isn't present.
    pub fn hex_in(&self, variant_id: &str) -> Option<u32> {
        self.per_variant
            .iter()
            .find(|(v, _)| *v == variant_id)
            .map(|(_, hex)| *hex)
            .or_else(|| self.per_variant.first().map(|(_, hex)| *hex))
    }
}

/// Synthetic variant id used for the "follow the OS" chip. Not a real
/// `Variant` in any theme's `variants` list — `apply()` resolves it to the
/// theme's `system_dark_variant` / `system_light_variant` based on the
/// current OS appearance, and on KDE Breeze it can additionally re-source
/// the whole palette from `~/.config/kdeglobals` so the player matches Plasma
/// exactly.
pub const SYSTEM_VARIANT_ID: &str = "system";

/// Synthetic accent id used for the "Material You" dot in the Material 3
/// theme's accent grid. Only valid when the user has picked a non-`None`
/// Color Style — the accent then follows whatever primary the dynamic
/// scheme produced from album art. Treated like [`SYSTEM_VARIANT_ID`]:
/// not in any theme's `accents` list, and `apply()` resolves it at paint
/// time to the dynamic accent that lives on `SystemColorState`.
pub const MATERIAL_YOU_ACCENT_ID: &str = "material_you";

#[derive(Clone, Copy, Debug)]
pub struct ThemeDef {
    pub id: &'static str,
    pub name: &'static str,
    pub variants: &'static [Variant],
    pub accents: &'static [AccentDef],
    pub default_variant: &'static str,
    pub default_accent: &'static str,
    /// True when this theme advertises a "System" chip alongside its
    /// declared variants. Selecting System maps to `system_dark_variant`
    /// or `system_light_variant` at apply time based on the OS signal.
    pub supports_system_mode: bool,
    pub system_dark_variant: &'static str,
    pub system_light_variant: &'static str,
}

impl ThemeDef {
    pub fn variant(&self, id: &str) -> Option<&'static Variant> {
        self.variants.iter().find(|v| v.id == id)
    }

    pub fn accent(&self, id: &str) -> Option<&'static AccentDef> {
        self.accents.iter().find(|a| a.id == id)
    }

    /// Resolved hex for `accent_id` in `variant_id`, with both lookups
    /// falling back to first-listed entries on miss.
    pub fn accent_hex(&self, accent_id: &str, variant_id: &str) -> Option<u32> {
        self.accent(accent_id).and_then(|a| a.hex_in(variant_id))
    }

    pub(crate) fn resolved_variant(&self, variant_id: &str) -> &'static Variant {
        self.variant(variant_id)
            .or_else(|| self.variant(self.default_variant))
            .unwrap_or(&self.variants[0])
    }

    pub(crate) fn resolved_accent_hex(&self, accent_id: &str, variant_id: &str) -> u32 {
        self.accent_hex(accent_id, variant_id)
            .or_else(|| self.accent_hex(self.default_accent, variant_id))
            .unwrap_or(self.resolved_variant(variant_id).palette.overlay1)
    }

    /// Map the `"system"` synthetic id to one of this theme's real variants
    /// based on `system_theme` (`"light"` / anything-else-treated-as-dark).
    /// Falls back to `default_variant` when the theme opts out of system
    /// mode or the named system pair isn't actually in `variants`.
    pub fn resolve_system_variant(&self, system_theme: &str) -> &'static Variant {
        if !self.supports_system_mode {
            return self.resolved_variant(self.default_variant);
        }
        let id = if system_theme == "light" {
            self.system_light_variant
        } else {
            self.system_dark_variant
        };
        self.resolved_variant(id)
    }
}
