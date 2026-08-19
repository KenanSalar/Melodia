//! Apply pipeline: resolve `(theme_id, variant_id, accent_id)` against the
//! registry and the OS / Material You signals, then write every theme-
//! dependent brush into the Slint `Theme` global. All Slint coupling for
//! the themes module lives here.

use slint::{Brush, Color, ComponentHandle};

use crate::AppWindow;
use crate::Theme as ThemeGlobal;

use super::palette::{MATERIAL_YOU_ACCENT_ID, Palette, SYSTEM_VARIANT_ID, ThemeDef};
use super::system_color_state::SystemColorState;

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
    let theme = super::get(theme_id);

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
            let palette = palette_from_kde(kde);
            let accent_hex = parse_hex_color(&kde.accent).unwrap_or(0x003d_aee9);
            // The *only* path pulling a real OS inactive-titlebar colour, so
            // our painted surfaces match the frame exactly on focus loss.
            let mantle_unfocused_hex = kde
                .colors
                .get("mantle_unfocused")
                .and_then(|s| parse_hex_color(s))
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

/// A `"#RRGGBB"` hex string as the packed `0x00RRGGBB` the palette tables use.
/// `None` on malformed input, which callers answer with a default.
#[cfg(target_os = "linux")]
fn parse_hex_color(s: &str) -> Option<u32> {
    let stripped = s.strip_prefix('#').unwrap_or(s);
    u32::from_str_radix(stripped, 16).ok()
}

/// A `Palette` from a parsed `kdeglobals` scheme: the structure slots straight
/// off the colours map, the three semantic ones from Plasma's own status
/// foregrounds.
///
/// The Breeze hexes below are a second line rather than the policy —
/// `kde_palette_from_sections` already substitutes the same defaults for a
/// scheme that omits a status foreground, and always hands back something
/// parseable. They fire only if that stops being true.
#[cfg(target_os = "linux")]
fn palette_from_kde(kde: &crate::services::system_theme::KdeColorPalette) -> Palette {
    let g =
        |key: &str| -> u32 { kde.colors.get(key).and_then(|s| parse_hex_color(s)).unwrap_or(0) };
    let overlay1 = g("overlay1");
    Palette {
        base: g("base"),
        mantle: g("mantle"),
        crust: g("crust"),
        surface0: g("surface0"),
        surface1: g("surface1"),
        surface2: g("surface2"),
        overlay0: g("overlay0"),
        overlay1,
        overlay2: g("overlay2"),
        text: g("text"),
        subtext0: g("subtext0"),
        subtext1: g("subtext1"),
        border: g("border"),
        red: parse_hex_color(&kde.red).unwrap_or(0x00da_4453),
        green: parse_hex_color(&kde.green).unwrap_or(0x0027_ae60),
        yellow: parse_hex_color(&kde.yellow).unwrap_or(0x00f6_7400),
    }
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
    // below. A no-op until the window is shown, which `main.rs`'s post-show
    // one-shot covers. See [`crate::services::dwm_titlebar`].
    #[cfg(target_os = "windows")]
    crate::services::dwm_titlebar::apply(ui, p.mantle);
}

/// A `0x00RRGGBB` value as an opaque solid `Brush`. `pub(crate)` because Now
/// Playing packs its per-artwork accent into a brush property too.
pub(crate) fn brush(rgb: u32) -> Brush {
    Brush::SolidColor(color(rgb))
}

/// The same as a bare `Color`, which the Now Playing gradient floor needs —
/// Slint's `.mix()` and gradient stops take `color`, not `brush`.
pub(crate) fn color(rgb: u32) -> Color {
    let r = ((rgb >> 16) & 0xff) as u8;
    let g = ((rgb >> 8) & 0xff) as u8;
    let b = (rgb & 0xff) as u8;
    Color::from_rgb_u8(r, g, b)
}

/// The same plus an alpha, for the two solved scrims. Their opacity is solved
/// per artwork, so baking it in keeps the Slint side one `background:` binding
/// rather than a colour plus a float the view has to recombine.
pub(crate) fn brush_with_alpha(rgb: u32, alpha: u8) -> Brush {
    Brush::SolidColor(color_with_alpha(rgb, f32::from(alpha) / 255.0))
}

/// [`color`] carrying a weight in its alpha, for a gradient stop that has to stay a `color`. The
/// aurora's tints arrive this way: `transparentize` on the Slint side multiplies rather than sets,
/// so the falloff shape and the per-artwork weight compose without either restating the other.
pub(crate) fn color_with_alpha(rgb: u32, alpha: f32) -> Color {
    color(rgb).with_alpha(alpha)
}

/// A solid `Brush` back to `0x00RRGGBB`, dropping alpha — how a solved surface
/// reads the theme accent's *hue* out of the global as an artwork-less fallback.
/// A gradient answers with its first stop, the right approximation here.
pub(crate) fn brush_to_rgb(brush: &Brush) -> u32 {
    color_to_rgb(brush.color())
}

/// Inverse of [`color`]. The Genre hero reads the hash-derived gradient stops
/// off `GenreRow` — they arrive as `Color`, never as a `Brush` — and has to
/// measure their lightness before it can solve a scrim against them.
pub(crate) fn color_to_rgb(c: Color) -> u32 {
    (u32::from(c.red()) << 16) | (u32::from(c.green()) << 8) | u32::from(c.blue())
}

/// sRGB luma weights, applied to the gamma-encoded channels rather than to
/// linearized ones — cheap, and the threshold below is tuned for it. Not the
/// relative luminance `ui::backdrop` solves scrims against; that one linearizes
/// first. Named because `theme.slint`'s `ink-on` spells the same four numbers
/// out and `themes::tests::theme_slint_ink_on_matches_on_accent_hex` builds its
/// expected Slint expression from these, so a drift on either side fails.
///
/// A third copy lives in `services::dwm_titlebar::is_dark_from_rgb`, duplicated
/// on purpose to keep that windows-only module off the cross-platform palette
/// code. Nothing pins it and nothing can — it's `cfg(windows)`, so a test for it
/// would never run in CI. Edit a weight here and edit that one by hand.
pub(super) const LUMA_R: f64 = 0.2126;
pub(super) const LUMA_G: f64 = 0.7152;
pub(super) const LUMA_B: f64 = 0.0722;
/// Above this, `fill` is light enough to take dark ink.
pub(super) const LUMA_THRESHOLD: f64 = 0.5;

/// Pick a contrast colour for text/icons rendered on top of `accent_hex`:
/// dark `#1e1e2e` for light accents, white for dark accents. Fast enough that
/// we don't bother caching per accent. f64 keeps clippy happy on the
/// u8 → float lift (channel values are 0..=255, well inside f64's range).
///
/// `theme.slint`'s `Theme.ink-on(brush)` is the Slint-side twin, for the
/// surfaces whose fill isn't the accent (`danger`, the traffic-light hues).
/// Same weights, same threshold, same pair — keep them in step.
pub(super) fn on_accent_hex(accent_hex: u32) -> u32 {
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

#[cfg(all(test, target_os = "linux"))]
#[path = "tests/apply_tests.rs"]
mod tests;
