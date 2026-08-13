//! Pluggable theme registry. Each theme is a static `ThemeDef` owning its
//! variants (palette + semantic colours) and accent definitions (one hex per
//! variant). `apply()` writes the resolved brushes into the Slint `Theme`
//! global; `accent_brushes()` builds the `[brush]` swatch list shown in the
//! settings picker.
//!
//! Palette data is ported verbatim from the legacy Tauri app
//! (`Melodia-tauri/src/themes/<name>.ts`). No heap allocation, no
//! `lazy_static` — themes are zero-cost static data referenced by
//! `&'static ThemeDef`.
//!
//! Module layout:
//! - [`palette`]: pure data structures (`Palette`, `Variant`, `AccentDef`,
//!   `ThemeDef`) and the synthetic `SYSTEM_VARIANT_ID` /
//!   `MATERIAL_YOU_ACCENT_ID` constants.
//! - [`system_color_state`]: bundled OS appearance signals (`SystemColorState`)
//!   consumed by `apply()`.
//! - [`mod@apply`]: the Slint-facing pipeline that writes palette brushes into
//!   the `Theme` global plus the colour-dot picker helper.
//! - Per-theme modules ([`catppuccin`], [`gnome`], [`kde`], [`macos`],
//!   [`material3`], [`windows`]): the static palette data.

pub mod catppuccin;
pub mod gnome;
pub mod kde;
pub mod macos;
pub mod material3;
pub mod windows;

pub mod apply;
pub mod palette;
pub mod system_color_state;

pub use apply::{accent_brushes, apply};
pub(crate) use apply::{brush, brush_to_rgb, brush_with_alpha, color, color_to_rgb};
pub use palette::{
    AccentDef, MATERIAL_YOU_ACCENT_ID, Palette, SYSTEM_VARIANT_ID, ThemeDef, Variant,
};
pub use system_color_state::SystemColorState;

static REGISTRY: &[&ThemeDef] = &[
    &catppuccin::CATPPUCCIN,
    &gnome::GNOME,
    &kde::KDE,
    &macos::MACOS,
    &material3::MATERIAL3,
    &windows::WINDOWS,
];

/// All registered themes in display order. Keep in sync with the chip order
/// the Settings UI shows.
pub fn registry() -> &'static [&'static ThemeDef] {
    REGISTRY
}

/// Find a theme by id; falls back to Catppuccin if the id is unknown.
pub fn get(id: &str) -> &'static ThemeDef {
    registry().iter().copied().find(|t| t.id == id).unwrap_or(&catppuccin::CATPPUCCIN)
}

/// Index of `theme_id` in [`registry`], or 0 (Catppuccin) on miss. Used by
/// `ui::appearance` to seed `Settings.theme-idx` from `SettingsData.theme_id`.
pub fn theme_index(theme_id: &str) -> usize {
    registry().iter().position(|t| t.id == theme_id).unwrap_or(0)
}

/// Index of `variant_id` in `theme.variants`, or 0 on miss.
pub fn variant_index(theme: &ThemeDef, variant_id: &str) -> usize {
    theme.variants.iter().position(|v| v.id == variant_id).unwrap_or(0)
}

/// Index of `accent_id` in `theme.accents`, or 0 on miss.
pub fn accent_index(theme: &ThemeDef, accent_id: &str) -> usize {
    theme.accents.iter().position(|a| a.id == accent_id).unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/registry_tests.rs"]
mod tests;
