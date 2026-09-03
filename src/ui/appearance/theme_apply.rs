//! Apply pipeline: resolve `(theme_id, variant_id, accent_id)` against the
//! registry and the OS / Material You signals, then write every theme-
//! dependent brush into the Slint `Theme` global.
//!
//! It lives here rather than under `themes/` because it is the only half of that directory that
//! names a Slint type, and carrying it there would put `melodia-ui` under every crate wanting a
//! palette. What it reads is `crate::themes`: tables, the registry, and the two derivations.

use slint::{Brush, Color, ComponentHandle};

use crate::themes::palette::{
    MATERIAL_YOU_ACCENT_ID, Palette, SYSTEM_VARIANT_ID, ThemeDef, on_accent_hex,
};
use crate::themes::system_color_state::SystemColorState;
use crate::{AppWindow, Theme as ThemeGlobal};

/// Brushes for the colour-dot picker — one per accent in `theme`, each
/// rendered in `variant_id`'s shade.
pub fn accent_brushes(theme: &ThemeDef, variant_id: &str) -> Vec<Brush> {
    theme.accents.iter().map(|a| brush(a.hex_in(variant_id).unwrap_or(0x88_88_88))).collect()
}

/// Resolve `(theme_id, variant_id, accent_id)` with fallbacks and write every
/// theme-dependent brush into the Slint `Theme` global.
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
    let theme = crate::themes::get(theme_id);

    // A dynamic palette wins over the static M3 variants whatever the variant
    // id, which is why this sits above the System branch. The accent picker
    // stays independent: `MATERIAL_YOU_ACCENT_ID` follows the dynamic primary,
    // and a static accent overrides only the accent, keeping dynamic surfaces.
    if theme_id == "material3"
        && let Some((palette, dyn_accent)) = &system.material_you
    {
        let accent_hex = if accent_id == MATERIAL_YOU_ACCENT_ID {
            *dyn_accent
        } else {
            let real_variant = if variant_id == SYSTEM_VARIANT_ID {
                theme.resolve_system_variant(&system.theme).id
            } else {
                variant_id
            };
            theme.accent_hex(accent_id, real_variant).unwrap_or(*dyn_accent)
        };
        // A palette generated from artwork has no OS inactive-titlebar colour
        // to offer, so the unfocused surface falls back to `base`.
        write_palette(ui, palette, accent_hex, palette.base);
        return;
    }

    if variant_id == SYSTEM_VARIANT_ID && theme.supports_system_mode {
        let resolved = theme.resolve_system_variant(&system.theme);

        // The one branch that bypasses the static palette. Compiled out
        // elsewhere, `KdeColorPalette` being Linux-only.
        #[cfg(target_os = "linux")]
        if theme_id == "kde-breeze"
            && let Some(kde) = &system.kde_palette
        {
            let palette = crate::themes::kde::palette_from_kde(kde);
            let accent_hex =
                crate::themes::kde::parse_hex_color(&kde.accent).unwrap_or(0x003d_aee9);
            // The *only* path pulling a real OS inactive-titlebar colour, so
            // our painted surfaces match the frame exactly on focus loss.
            let mantle_unfocused_hex = kde
                .colors
                .get("mantle_unfocused")
                .and_then(|s| crate::themes::kde::parse_hex_color(s))
                .unwrap_or(palette.base);
            write_palette(ui, &palette, accent_hex, mantle_unfocused_hex);
            return;
        }

        let accent_hex = theme.resolved_accent_hex(accent_id, resolved.id);
        write_palette(ui, &resolved.palette, accent_hex, resolved.palette.base);
        return;
    }

    let variant = theme.resolved_variant(variant_id);
    let accent_hex = theme.resolved_accent_hex(accent_id, variant.id);
    // A static variant has no OS source for an inactive titlebar either.
    write_palette(ui, &variant.palette, accent_hex, variant.palette.base);
}

fn write_palette(ui: &AppWindow, p: &Palette, accent_hex: u32, mantle_unfocused_hex: u32) {
    let g = ui.global::<ThemeGlobal>();

    // Surfaces / structure
    g.set_base(brush(p.base));
    g.set_mantle(brush(p.mantle));
    g.set_mantle_unfocused(brush(mantle_unfocused_hex));
    g.set_crust(brush(p.crust));
    g.set_surface0(brush(p.surface0));
    g.set_surface1(brush(p.surface1));
    g.set_surface2(brush(p.surface2));

    // Overlays / text
    g.set_overlay0(brush(p.overlay0));
    g.set_overlay1(brush(p.overlay1));
    g.set_overlay2(brush(p.overlay2));
    g.set_text(brush(p.text));
    g.set_subtext0(brush(p.subtext0));
    g.set_subtext1(brush(p.subtext1));
    g.set_border(brush(p.border));

    // Semantic palette slots
    g.set_red(brush(p.red));
    g.set_green(brush(p.green));
    g.set_yellow(brush(p.yellow));

    // Accent + on-accent text
    g.set_accent(brush(accent_hex));
    g.set_accent_text(brush(on_accent_hex(accent_hex)));

    // `danger` / `danger-text` stay bound to red / accent-text via the Slint
    // declarative defaults — re-evaluated whenever the source brushes update,
    // so we don't write them here.

    // Paint the OS-drawn caption in the same mantle so it blends into the chrome
    // below. Nothing to paint until the window is shown, which `main.rs`'s
    // post-show one-shot covers. See [`crate::services::dwm_titlebar`].
    #[cfg(target_os = "windows")]
    if let Some(hwnd) = crate::ui::window_chrome::win32_hwnd(ui) {
        crate::services::dwm_titlebar::apply(hwnd, p.mantle);
    }
}

/// Read `Theme.mantle` back off the Slint global and repaint the OS caption from it.
///
/// Called once at startup after the window is shown, the boot-time [`apply`] having had no `HWND`
/// to paint; every later palette write drives the DWM call from `write_palette` directly. It reads
/// the global rather than taking a `Palette` because by then the resolved one is only on the Slint
/// side — which is also why it is here and not in `dwm_titlebar`, whose half of this names no
/// Slint type.
#[cfg(target_os = "windows")]
pub fn reapply_from_theme(app: &AppWindow) {
    let Some(hwnd) = crate::ui::window_chrome::win32_hwnd(app) else {
        return;
    };
    let mantle = color_to_rgb(app.global::<ThemeGlobal>().get_mantle().color());
    crate::services::dwm_titlebar::apply(hwnd, mantle);
}

/// A `0x00RRGGBB` value as an opaque solid `Brush`. `pub(crate)` because Now
/// Playing packs its per-artwork accent into a brush property too.
pub fn brush(rgb: u32) -> Brush {
    Brush::SolidColor(color(rgb))
}

/// The same as a bare `Color`, which the Now Playing gradient floor needs —
/// Slint's `.mix()` and gradient stops take `color`, not `brush`.
pub fn color(rgb: u32) -> Color {
    let r = ((rgb >> 16) & 0xff) as u8;
    let g = ((rgb >> 8) & 0xff) as u8;
    let b = (rgb & 0xff) as u8;
    Color::from_rgb_u8(r, g, b)
}

/// The same plus an alpha, for the two solved scrims. Their opacity is solved
/// per artwork, so baking it in keeps the Slint side one `background:` binding
/// rather than a colour plus a float the view has to recombine.
pub fn brush_with_alpha(rgb: u32, alpha: u8) -> Brush {
    Brush::SolidColor(color_with_alpha(rgb, f32::from(alpha) / 255.0))
}

/// [`color`] carrying a weight in its alpha, for a gradient stop that has to stay a `color`. The
/// aurora's tints arrive this way: `transparentize` on the Slint side multiplies rather than sets,
/// so the falloff shape and the per-artwork weight compose without either restating the other.
pub fn color_with_alpha(rgb: u32, alpha: f32) -> Color {
    color(rgb).with_alpha(alpha)
}

/// A solid `Brush` back to `0x00RRGGBB`, dropping alpha — how a solved surface
/// reads the theme accent's *hue* out of the global as an artwork-less fallback.
/// A gradient answers with its first stop, the right approximation here.
pub fn brush_to_rgb(brush: &Brush) -> u32 {
    color_to_rgb(brush.color())
}

/// Inverse of [`color`]. The Genre hero reads the hash-derived gradient stops
/// off `GenreRow` — they arrive as `Color`, never as a `Brush` — and has to
/// measure their lightness before it can solve a scrim against them.
pub fn color_to_rgb(c: Color) -> u32 {
    (u32::from(c.red()) << 16) | (u32::from(c.green()) << 8) | u32::from(c.blue())
}

#[cfg(test)]
#[path = "tests/theme_apply_tests.rs"]
mod tests;
