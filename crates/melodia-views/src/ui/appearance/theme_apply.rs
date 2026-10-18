//! Apply pipeline: resolve `(theme_id, variant_id, accent_id)` against the
//! registry and the OS / Material You signals, then hand the palette to
//! [`theme_paint`](super::theme_paint), which writes it into the Slint `Theme` global.
//!
//! It lives here rather than under `themes/` because it is the only half of that directory that
//! names a Slint type, and carrying it there would put `melodia-ui` under every crate wanting a
//! palette. What it reads is `melodia_core::themes`: tables, the registry, and the two derivations.

use slint::Brush;

use melodia_core::themes::material3::MATERIAL3;
use melodia_core::themes::palette::{MATERIAL_YOU_ACCENT_ID, ThemeDef, Variant};
use melodia_core::themes::system_color_state::SystemColorState;
use melodia_ui::AppWindow;

use super::rgb::brush;
use super::theme_paint::{self, Painted, Source};

/// Brushes for the colour-dot picker — one per accent in `theme`, each
/// rendered in `variant_id`'s shade.
pub fn accent_brushes(theme: &ThemeDef, variant_id: &str) -> Vec<Brush> {
    theme.accents.iter().map(|a| brush(a.hex_in(variant_id).unwrap_or(0x88_88_88))).collect()
}

/// Resolve `(theme_id, variant_id, accent_id)` with fallbacks and paint the
/// palette it lands on.
///
/// The synthetic system variant maps onto one of the theme's real ones, for a
/// theme that opts in. KDE Breeze additionally bypasses its static palette and
/// re-sources every slot from `kdeglobals`, so the player matches Plasma's
/// active scheme exactly; everywhere else the OS only picks dark or light.
pub fn apply(
    ui: &AppWindow,
    theme_id: &str,
    variant_id: &str,
    accent_id: &str,
    system: &SystemColorState,
) {
    let theme = melodia_core::themes::get(theme_id);

    // A dynamic palette wins over the static M3 variants whatever the variant
    // id, which is why this sits above the System branch. The accent picker
    // stays independent: `MATERIAL_YOU_ACCENT_ID` follows the dynamic primary,
    // and a static accent overrides only the accent, keeping dynamic surfaces.
    if theme_id == MATERIAL3.id
        && let Some((palette, dyn_accent)) = &system.material_you
    {
        let accent_hex = if accent_id == MATERIAL_YOU_ACCENT_ID {
            *dyn_accent
        } else {
            let real_variant = theme.shade_for(variant_id, &system.theme).id;
            theme.accent_hex(accent_id, real_variant).unwrap_or(*dyn_accent)
        };
        // A palette generated from artwork has no OS inactive-titlebar colour
        // to offer, so the unfocused surface falls back to `base`.
        let painted = Painted::new(*palette, accent_hex, palette.base);
        theme_paint::paint(ui, painted, Source::Dynamic);
        return;
    }

    // The one branch that bypasses the static palette. Compiled out
    // elsewhere, `KdeColorPalette` being Linux-only.
    #[cfg(target_os = "linux")]
    if variant_id == melodia_core::themes::palette::SYSTEM_VARIANT_ID
        && theme.supports_system_mode
        && theme_id == melodia_core::themes::kde::KDE.id
        && let Some(kde) = &system.kde_palette
    {
        let palette = melodia_core::themes::kde::palette_from_kde(kde);
        let accent_hex =
            melodia_core::themes::kde::parse_hex_color(&kde.accent).unwrap_or(0x003d_aee9);
        // The one path reading the live OS inactive-titlebar colour, so
        // our painted surfaces match the frame exactly on focus loss.
        let mantle_unfocused_hex = kde
            .colors
            .get("mantle_unfocused")
            .and_then(|s| melodia_core::themes::kde::parse_hex_color(s))
            .unwrap_or(palette.base);
        let painted = Painted::new(palette, accent_hex, mantle_unfocused_hex);
        theme_paint::paint(ui, painted, Source::Static);
        return;
    }

    let variant = theme.shade_for(variant_id, &system.theme);
    let accent_hex = theme.resolved_accent_hex(accent_id, variant.id);
    let painted = Painted::new(variant.palette, accent_hex, unfocused_mantle(variant));
    theme_paint::paint(ui, painted, Source::Static);
}

fn unfocused_mantle(variant: &Variant) -> u32 {
    variant.mantle_unfocused.unwrap_or(variant.palette.base)
}

#[cfg(test)]
#[path = "tests/theme_apply_tests.rs"]
mod tests;
