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
    theme
        .accents
        .iter()
        .map(|a| brush(a.hex_in(variant_id).unwrap_or(0x88_88_88)))
        .collect()
}

/// Resolve `(theme_id, variant_id, accent_id)` (with fallbacks) and write
/// every theme-dependent brush into the Slint `Theme` global.
///
/// When `variant_id == SYSTEM_VARIANT_ID` and the theme opts in via
/// `supports_system_mode`, the synthetic id is mapped to one of the
/// theme's real variants based on `system.theme`. KDE Breeze additionally
/// bypasses its static Light/Dark palette and re-sources the 22 brushes
/// from the cached `kdeglobals` palette so the player matches Plasma's
/// active colour scheme exactly. All other themes use their declared
/// system pair palette unchanged — the OS only picks dark vs. light there.
pub fn apply(
    ui: &AppWindow,
    theme_id: &str,
    variant_id: &str,
    accent_id: &str,
    system: &SystemColorState,
) {
    let theme = super::get(theme_id);

    // Material You: when the M3 coordinator has produced a dynamic palette
    // for the current artwork, that palette wins over the static M3
    // variants regardless of Dark / Light / System. The accent picker is
    // independent — `MATERIAL_YOU_ACCENT_ID` follows the dynamic primary,
    // any of the 8 static accents overrides just the accent while
    // keeping dynamic surfaces. Placed before the System branch so
    // `variant_id == "system"` still goes through dynamic colour.
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
            theme
                .accent_hex(accent_id, real_variant)
                .unwrap_or(*dyn_accent)
        };
        // Material You has no OS-defined "inactive titlebar" concept —
        // the dynamic palette is generated from artwork, not the WM —
        // so the unfocused surface falls back to `base`, same visual
        // as the pre-feature behaviour.
        write_palette(ui, palette, accent_hex, palette.base);
        return;
    }

    if variant_id == SYSTEM_VARIANT_ID && theme.supports_system_mode {
        let resolved = theme.resolve_system_variant(&system.theme);

        // KDE OS-colour override is the only branch that bypasses the
        // static palette. Compiled out on non-Linux because
        // `KdeColorPalette` lives behind `#[cfg(target_os = "linux")]`.
        #[cfg(target_os = "linux")]
        if theme_id == "kde-breeze"
            && let Some(kde) = &system.kde_palette
        {
            let palette = palette_from_kde(kde);
            let accent_hex = parse_hex_color(&kde.accent).unwrap_or(0x003d_aee9);
            // KDE+System is the *only* path that pulls a real OS
            // inactive-titlebar colour — `get_kde_colors()` reads
            // `[WM] inactiveBackground` and stores it under
            // `mantle_unfocused`. The user's KDE colour scheme drives
            // the unfocused tint, so our painted surfaces match the
            // OS frame exactly when the window loses focus.
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
    // Static variants (every non-KDE+System path) have no OS source for
    // an inactive titlebar — fall back to `base`, the same visual as
    // before this feature shipped.
    write_palette(ui, &variant.palette, accent_hex, variant.palette.base);
}

/// Parse a `"#RRGGBB"` (or `"RRGGBB"`) hex string into a packed `0x00RRGGBB`
/// `u32` matching the rest of the palette tables. Returns `None` for
/// malformed input — callers fall back to a sensible default.
///
/// Only `palette_from_kde` (Linux KDE) calls this; cfg-gated to match.
#[cfg(target_os = "linux")]
fn parse_hex_color(s: &str) -> Option<u32> {
    let stripped = s.strip_prefix('#').unwrap_or(s);
    u32::from_str_radix(stripped, 16).ok()
}

/// Build a `Palette` from a parsed `kdeglobals` colour scheme. The 13 base /
/// structure slots come directly from the `KdeColorPalette::colors` map
/// (`get_kde_colors()` already synthesizes the entries we need). `red`
/// comes from the dedicated `red` field. The six unused semantic slots
/// (green / yellow / peach / mauve / pink / lavender) collapse to
/// `overlay1` via `Palette::fallback_semantics` — same approach as every
/// non-Catppuccin theme — so any component reading `Theme.green` etc.
/// stays muted-but-on-palette instead of fluorescent.
#[cfg(target_os = "linux")]
fn palette_from_kde(kde: &crate::services::system_theme::KdeColorPalette) -> Palette {
    let g = |key: &str| -> u32 {
        kde.colors
            .get(key)
            .and_then(|s| parse_hex_color(s))
            .unwrap_or(0)
    };
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
        ..Palette::fallback_semantics(overlay1)
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
    g.set_peach(brush(p.peach));
    g.set_mauve(brush(p.mauve));
    g.set_pink(brush(p.pink));
    g.set_lavender(brush(p.lavender));

    // Accent + on-accent text
    g.set_accent(brush(accent_hex));
    g.set_accent_text(brush(on_accent_hex(accent_hex)));

    // `danger`, `danger-hover`, `danger-text` stay bound to red / peach /
    // accent-text via the Slint declarative defaults — re-evaluated whenever
    // the source brushes update, so we don't write them here.

    // Windows: paint the OS-drawn caption with the same mantle colour so
    // it blends into the chrome below, and flip the dark/light variant to
    // match the resolved theme. No-op until the window has been shown
    // (HWND only exists after `app.show()`) — `main.rs` fires a follow-up
    // apply from a post-show `invoke_from_event_loop` to cover the boot
    // path. See [`crate::services::dwm_titlebar`].
    #[cfg(target_os = "windows")]
    crate::services::dwm_titlebar::apply(ui, p.mantle);
}

/// Pack a `0x00RRGGBB` value into an opaque solid `Brush`. Exposed at
/// `pub(crate)` because the Now Playing view also packs a per-artwork
/// accent (extracted via `services::material_you::extract_source_argb_from_rgb8`)
/// into a Slint brush property.
pub(crate) fn brush(rgb: u32) -> Brush {
    let r = ((rgb >> 16) & 0xff) as u8;
    let g = ((rgb >> 8) & 0xff) as u8;
    let b = (rgb & 0xff) as u8;
    Brush::SolidColor(Color::from_rgb_u8(r, g, b))
}

/// Pick a contrast colour for text/icons rendered on top of `accent_hex`:
/// dark `#1e1e2e` for light accents, white for dark accents. Uses the
/// standard sRGB relative-luminance threshold of 0.5 — fast enough that we
/// don't bother caching per accent. f64 keeps clippy happy on the
/// u8 → float lift (channel values are 0..=255, well inside f64's range).
pub(super) fn on_accent_hex(accent_hex: u32) -> u32 {
    let r = f64::from((accent_hex >> 16) & 0xff) / 255.0;
    let g = f64::from((accent_hex >> 8) & 0xff) / 255.0;
    let b = f64::from(accent_hex & 0xff) / 255.0;
    let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    if lum > 0.5 { 0x001e_1e2e } else { 0x00ff_ffff }
}
