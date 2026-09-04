//! Pluggable theme registry. Each theme is a static `ThemeDef` owning its
//! variants (palette + semantic colours) and accent definitions (one hex per
//! variant).
//!
//! Palette data is ported verbatim from the legacy Tauri app
//! (`Melodia-tauri/src/themes/<name>.ts`). No heap allocation, no
//! `lazy_static` — themes are zero-cost static data referenced by
//! `&'static ThemeDef`.
//!
//! **Nothing under this directory names a Slint type.** The half that does is
//! `ui::appearance::theme_apply`, which resolves a triple against this registry
//! and writes the brushes into the `Theme` global; keeping it there is what
//! stops every crate wanting a palette from carrying `melodia-ui`.
//!
//! Module layout:
//! - [`palette`]: the data structures (`Palette`, `Variant`, `AccentDef`,
//!   `ThemeDef`), the synthetic `SYSTEM_VARIANT_ID` / `MATERIAL_YOU_ACCENT_ID`
//!   constants, and the luminance split `on_accent_hex` reads off a colour.
//! - [`system_color_state`]: bundled OS appearance signals (`SystemColorState`).
//! - Per-theme modules ([`catppuccin`], [`gnome`], [`kde`], [`macos`],
//!   [`material3`], [`windows`]): the static palette data. [`kde`] carries a
//!   second, dynamic derivation off the live `kdeglobals`.

pub mod catppuccin;
pub mod gnome;
pub mod kde;
pub mod macos;
pub mod material3;
pub mod windows;

pub mod palette;
pub mod system_color_state;

pub use palette::{
    AccentDef, MATERIAL_YOU_ACCENT_ID, Palette, SYSTEM_VARIANT_ID, ThemeDef, Variant, on_accent_hex,
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
