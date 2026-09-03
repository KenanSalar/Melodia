//! Pure data structures for the theme registry, and the luminance split read off them. No Slint
//! imports and no OS signals: the brush writes live in `ui::appearance::theme_apply`, the OS
//! signals in `system_color_state.rs`.

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

/// sRGB luma weights, applied to the gamma-encoded channels rather than to
/// linearized ones — cheap, and the threshold below is tuned for it. Not the
/// relative luminance `ui::backdrop` solves scrims against; that one linearizes
/// first. Named because `theme.slint`'s `ink-on` spells the same four numbers
/// out and `themes::tests::theme_slint_is_light_matches_on_accent_hex` builds
/// its expected Slint expression from these, so a drift on either side fails.
///
/// A third copy lives in `services::dwm_titlebar::is_dark_from_rgb`, duplicated
/// on purpose to keep that windows-only module off the palette code that calls
/// into it. It is pinned against `on_accent_hex` rather than against these, by
/// `services::dwm_titlebar::tests` — which runs only under the
/// `test-windows` job, the reason that copy went unchecked for so long.
pub const LUMA_R: f64 = 0.2126;
pub const LUMA_G: f64 = 0.7152;
pub const LUMA_B: f64 = 0.0722;
/// Above this, `fill` is light enough to take dark ink.
pub const LUMA_THRESHOLD: f64 = 0.5;

/// Pick a contrast colour for text/icons rendered on top of `accent_hex`:
/// dark `#1e1e2e` for light accents, white for dark accents. Fast enough that
/// we don't bother caching per accent. f64 keeps clippy happy on the
/// u8 → float lift (channel values are 0..=255, well inside f64's range).
///
/// `theme.slint`'s `Theme.ink-on(brush)` is the Slint-side twin, for the
/// surfaces whose fill isn't the accent (`danger`, the traffic-light hues).
/// Same weights, same threshold, same pair — keep them in step.
///
pub fn on_accent_hex(accent_hex: u32) -> u32 {
    let r = f64::from((accent_hex >> 16) & 0xff) / 255.0;
    let g = f64::from((accent_hex >> 8) & 0xff) / 255.0;
    let b = f64::from(accent_hex & 0xff) / 255.0;
    let lum = LUMA_R * r + LUMA_G * g + LUMA_B * b;
    if lum > LUMA_THRESHOLD {
        0x001e_1e2e
    } else {
        0x00ff_ffff
    }
}
