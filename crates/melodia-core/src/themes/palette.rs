//! Pure data structures for the theme registry, and the luminance split read off them. No Slint
//! imports and no OS signals: the brush writes live in `ui::appearance::theme_paint`, the OS
//! signals in `system_color_state.rs`.

/// The brush slots a palette table fills, as `0x00RRGGBB` so the tables read as the hex they are.
/// `theme_paint` writes three more: `mantle_unfocused`, and `accent` / `accent_text`, picked apart
/// from the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    // 2 card controls: a control resting on a `surface0` card, and its hover. Slots of their own
    // because most palettes keep `surface1` all but level with the card.
    pub card_control: u32,
    pub card_control_hover: u32,
    // 3 semantic, named by every palette with no struct-update fallback, so one that forgets a
    // slot fails to compile rather than greying the traffic lights, toasts and stars.
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
}

/// How far `card_control` sits from `surface0` toward `text`, in percent: what a generated
/// palette fills the slot with, and where a hand-written table starts before it is tuned.
pub const CARD_CONTROL_PERCENT: u32 = 20;
/// [`CARD_CONTROL_PERCENT`]'s twin for `card_control_hover`.
pub const CARD_CONTROL_HOVER_PERCENT: u32 = 30;

/// Returns `from` moved `percent` of the way to `to`, each channel rounded to the nearest step;
/// past 100 it lands on `to`. Toward a palette's `text` it lifts a dark surface and sinks a light
/// one, so no caller has to ask which the palette is.
pub fn toward(from: u32, to: u32, percent: u32) -> u32 {
    let percent = percent.min(100);
    let channel = |shift: u32| {
        let (a, b) = ((from >> shift) & 0xff, (to >> shift) & 0xff);
        (a * (100 - percent) + b * percent + 50) / 100
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[derive(Clone, Copy, Debug)]
pub struct Variant {
    pub id: &'static str,
    pub name: &'static str,
    pub palette: Palette,
    /// The inactive titlebar of the OS scheme this variant copies, which an unfocused window's
    /// chrome fades to. `None` where no scheme is copied, and the chrome takes `base`.
    pub mantle_unfocused: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct AccentDef {
    pub id: &'static str,
    pub name: &'static str,
    /// `(variant-id, 0xRRGGBB)` pairs. Linear scan — at most 4 entries.
    pub per_variant: &'static [(&'static str, u32)],
}

impl AccentDef {
    /// Falls back to the first listed variant.
    pub fn hex_in(&self, variant_id: &str) -> Option<u32> {
        self.per_variant
            .iter()
            .find(|(v, _)| *v == variant_id)
            .map(|(_, hex)| *hex)
            .or_else(|| self.per_variant.first().map(|(_, hex)| *hex))
    }
}

/// The "follow the OS" chip, in no theme's `variants`: `apply()` resolves it to the theme's system
/// pair, and on KDE Breeze may re-source the whole palette from `kdeglobals`.
pub const SYSTEM_VARIANT_ID: &str = "system";

/// The Material You dot, valid only under a Color Style and in no theme's `accents`; resolved at
/// paint time to the accent `SystemColorState` holds.
pub const MATERIAL_YOU_ACCENT_ID: &str = "material_you";

/// The Melodia mark's two gradient stops on a light surface, as `0x00RRGGBB`: Catppuccin Latte's
/// blue and teal. `theme.slint`'s `brand-mark` spells the same pair for the titlebar, and the tray
/// icon paints it on a light taskbar or panel, which no Slint brush reaches.
pub const BRAND_MARK_ON_LIGHT: [u32; 2] = [0x001e_66f5, 0x0017_9299];

#[derive(Clone, Copy, Debug)]
pub struct ThemeDef {
    pub id: &'static str,
    pub name: &'static str,
    pub variants: &'static [Variant],
    pub accents: &'static [AccentDef],
    pub default_variant: &'static str,
    pub default_accent: &'static str,
    /// Whether the theme offers a "System" chip, mapped to its system pair at apply time.
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

    /// Both lookups fall back to the first listed entry.
    pub fn accent_hex(&self, accent_id: &str, variant_id: &str) -> Option<u32> {
        self.accent(accent_id).and_then(|a| a.hex_in(variant_id))
    }

    pub fn resolved_variant(&self, variant_id: &str) -> &'static Variant {
        self.variant(variant_id)
            .or_else(|| self.variant(self.default_variant))
            .unwrap_or(&self.variants[0])
    }

    pub fn resolved_accent_hex(&self, accent_id: &str, variant_id: &str) -> u32 {
        self.accent_hex(accent_id, variant_id)
            .or_else(|| self.accent_hex(self.default_accent, variant_id))
            .unwrap_or(self.resolved_variant(variant_id).palette.overlay1)
    }

    /// `"light"` picks the light variant and anything else the dark; `default_variant` where the
    /// theme opts out of system mode or its pair is missing.
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

    /// The real variant `variant_id` paints with: [`SYSTEM_VARIANT_ID`] resolved against
    /// `system_theme`, anything else through [`Self::resolved_variant`].
    pub fn shade_for(&self, variant_id: &str, system_theme: &str) -> &'static Variant {
        if variant_id == SYSTEM_VARIANT_ID {
            self.resolve_system_variant(system_theme)
        } else {
            self.resolved_variant(variant_id)
        }
    }
}

/// sRGB luma on the gamma-encoded channels, not the linearized luminance `ui::backdrop` solves
/// against. Named because `theme.slint`'s `ink-on` spells the same numbers, pinned by
/// `themes::tests`; `dwm_titlebar::is_dark_from_rgb` is the third copy, pinned by its own tests.
pub const LUMA_R: f64 = 0.2126;
pub const LUMA_G: f64 = 0.7152;
pub const LUMA_B: f64 = 0.0722;
/// Above this, `fill` is light enough to take dark ink.
pub const LUMA_THRESHOLD: f64 = 0.5;

pub fn is_light_hex(fill_hex: u32) -> bool {
    let r = f64::from((fill_hex >> 16) & 0xff) / 255.0;
    let g = f64::from((fill_hex >> 8) & 0xff) / 255.0;
    let b = f64::from(fill_hex & 0xff) / 255.0;
    LUMA_R * r + LUMA_G * g + LUMA_B * b > LUMA_THRESHOLD
}

/// Dark ink on a light accent, white on a dark one. `Theme.ink-on` is the Slint twin for fills
/// that aren't the accent; keep the two in step.
pub fn on_accent_hex(accent_hex: u32) -> u32 {
    if is_light_hex(accent_hex) { 0x001e_1e2e } else { 0x00ff_ffff }
}
