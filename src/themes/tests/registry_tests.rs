// Theme tests verify hex literals byte-for-byte; underscores would obscure
// the match against the upstream Tauri sources.
#![allow(clippy::unreadable_literal)]

use super::*;
use crate::themes::apply::on_accent_hex;

#[test]
fn registry_lists_six_themes_in_display_order() {
    let ids: Vec<_> = registry().iter().map(|t| t.id).collect();
    assert_eq!(
        ids,
        vec!["catppuccin", "gnome-adwaita", "kde-breeze", "macos", "material3", "windows-fluent"],
    );
}

#[test]
fn unknown_theme_id_falls_back_to_catppuccin() {
    assert_eq!(get("not-a-theme").id, "catppuccin");
}

#[test]
fn catppuccin_mocha_mauve_matches_tauri_hex() {
    let theme = get("catppuccin");
    assert_eq!(theme.accent_hex("mauve", "mocha"), Some(0xcba6f7));
    assert_eq!(theme.accent_hex("mauve", "latte"), Some(0x8839ef));
    assert_eq!(theme.accent_hex("mauve", "frappe"), Some(0xca9ee6));
}

#[test]
fn unknown_accent_or_variant_returns_none_or_first_variant() {
    let theme = get("catppuccin");
    assert_eq!(theme.accent_hex("not-an-accent", "mocha"), None);
    // Unknown variant on a known accent: falls back to the first variant
    // listed for that accent (latte for Catppuccin).
    assert_eq!(theme.accent_hex("mauve", "not-a-variant"), Some(0x8839ef));
}

#[test]
fn gnome_dark_semantics_match_adwaita_tokens() {
    // Adwaita's error / warning / success tokens.
    let gnome = get("gnome-adwaita");
    let dark = gnome.variant("dark");
    assert!(dark.is_some(), "gnome dark variant must exist");
    let Some(dark) = dark else { return };
    let p = &dark.palette;
    assert_eq!(p.red, 0xc01c28);
    assert_eq!(p.green, 0x57e389);
    assert_eq!(p.yellow, 0xf6d32d);
}

#[test]
fn every_variant_defines_three_distinct_semantic_colours() {
    // The regression guard for the grey traffic lights: `red` / `green` /
    // `yellow` drive the macOS-style titlebar cluster, the success / warning
    // toasts and the star rating, so a palette that leaves one sitting on a
    // neutral from the surface ramp turns those signals grey. There is no
    // struct-update fallback left to make that happen silently, but a
    // hand-written table can still paste the wrong hex in.
    for theme in registry() {
        for variant in theme.variants {
            let p = &variant.palette;
            let where_ = format!("{}/{}", theme.id, variant.id);
            for (name, semantic) in [("red", p.red), ("green", p.green), ("yellow", p.yellow)] {
                for (neutral_name, neutral) in [
                    ("overlay0", p.overlay0),
                    ("overlay1", p.overlay1),
                    ("overlay2", p.overlay2),
                ] {
                    assert_ne!(
                        semantic, neutral,
                        "{where_}: {name} must not be the {neutral_name} neutral",
                    );
                }
            }
            assert_ne!(p.red, p.green, "{where_}: red and green must differ");
            assert_ne!(p.red, p.yellow, "{where_}: red and yellow must differ");
            assert_ne!(p.green, p.yellow, "{where_}: green and yellow must differ");
        }
    }
}

#[test]
fn theme_index_matches_registry_order() {
    assert_eq!(theme_index("catppuccin"), 0);
    assert_eq!(theme_index("gnome-adwaita"), 1);
    assert_eq!(theme_index("kde-breeze"), 2);
    assert_eq!(theme_index("macos"), 3);
    assert_eq!(theme_index("material3"), 4);
    assert_eq!(theme_index("windows-fluent"), 5);
    // Unknown id falls back to 0.
    assert_eq!(theme_index("not-a-theme"), 0);
}

#[test]
fn variant_and_accent_index_lookups_round_trip() {
    let theme = get("catppuccin");
    assert_eq!(variant_index(theme, "mocha"), 3);
    assert_eq!(variant_index(theme, "latte"), 0);
    assert_eq!(variant_index(theme, "not-a-variant"), 0);

    assert_eq!(accent_index(theme, "mauve"), 3);
    assert_eq!(accent_index(theme, "rosewater"), 0);
    assert_eq!(accent_index(theme, "not-an-accent"), 0);
}

#[test]
fn accent_brushes_returns_one_per_accent() {
    let theme = get("catppuccin");
    let brushes = accent_brushes(theme, "mocha");
    assert_eq!(brushes.len(), 14);

    let gnome = get("gnome-adwaita");
    assert_eq!(accent_brushes(gnome, "dark").len(), 9);

    let macos = get("macos");
    assert_eq!(accent_brushes(macos, "light").len(), 8);
}

#[test]
fn on_accent_picks_dark_text_for_light_accent() {
    // Light accent (Catppuccin Mocha mauve = #cba6f7) → dark text.
    assert_eq!(on_accent_hex(0xcba6f7), 0x001e1e2e);
    // Dark accent (Catppuccin Latte mauve = #8839ef) → light text.
    assert_eq!(on_accent_hex(0x8839ef), 0x00ffffff);
    // Pure white → dark text; pure black → light text.
    assert_eq!(on_accent_hex(0x00ffffff), 0x001e1e2e);
    assert_eq!(on_accent_hex(0x00000000), 0x00ffffff);
}

#[test]
fn resolve_system_variant_picks_dark_or_light_pair() {
    // Catppuccin maps System → Mocha (dark) / Latte (light).
    let cat = get("catppuccin");
    assert!(cat.supports_system_mode);
    assert_eq!(cat.resolve_system_variant("dark").id, "mocha");
    assert_eq!(cat.resolve_system_variant("light").id, "latte");
    // Unknown OS theme strings are treated as dark — same as the XDG
    // portal's "no preference" value.
    assert_eq!(cat.resolve_system_variant("").id, "mocha");

    // Two-variant themes (KDE, GNOME, …) reuse their canonical ids.
    let kde = get("kde-breeze");
    assert_eq!(kde.resolve_system_variant("dark").id, "dark");
    assert_eq!(kde.resolve_system_variant("light").id, "light");
}

#[test]
fn every_theme_default_variant_and_accent_resolve() {
    for theme in registry() {
        assert!(
            theme.variant(theme.default_variant).is_some(),
            "{}: default_variant {:?} not in variants",
            theme.id,
            theme.default_variant,
        );
        assert!(
            theme.accent(theme.default_accent).is_some(),
            "{}: default_accent {:?} not in accents",
            theme.id,
            theme.default_accent,
        );
        // The default accent must have a hex defined for the default variant.
        assert!(
            theme.accent_hex(theme.default_accent, theme.default_variant).is_some(),
            "{}: default accent hex missing for default variant",
            theme.id,
        );
    }
}
