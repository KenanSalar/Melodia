use super::*;

// ── color_scheme_to_str ──

#[test]
fn color_scheme_no_preference_is_dark() {
    assert_eq!(color_scheme_to_str(0), "dark");
}

#[test]
fn color_scheme_prefer_dark() {
    assert_eq!(color_scheme_to_str(1), "dark");
}

#[test]
fn color_scheme_prefer_light() {
    assert_eq!(color_scheme_to_str(2), "light");
}

#[test]
fn color_scheme_unknown_value_is_dark() {
    assert_eq!(color_scheme_to_str(99), "dark");
}

// ── parse_rgb ──

#[test]
fn parse_rgb_valid() {
    assert_eq!(parse_rgb("255,128,0"), Some((255, 128, 0)));
}

#[test]
fn parse_rgb_with_whitespace() {
    assert_eq!(parse_rgb(" 255 , 128 , 0 "), Some((255, 128, 0)));
}

#[test]
fn parse_rgb_too_few_parts() {
    assert_eq!(parse_rgb("255,128"), None);
}

#[test]
fn parse_rgb_too_many_parts() {
    assert_eq!(parse_rgb("255,128,0,255"), None);
}

#[test]
fn parse_rgb_non_numeric() {
    assert_eq!(parse_rgb("abc,128,0"), None);
}

#[test]
fn parse_rgb_empty() {
    assert_eq!(parse_rgb(""), None);
}

// ── rgb_to_hex ──

#[test]
fn rgb_to_hex_black() {
    assert_eq!(rgb_to_hex(0, 0, 0), "#000000");
}

#[test]
fn rgb_to_hex_white() {
    assert_eq!(rgb_to_hex(255, 255, 255), "#ffffff");
}

#[test]
fn rgb_to_hex_orange() {
    assert_eq!(rgb_to_hex(255, 128, 0), "#ff8000");
}

// ── blend ──

#[test]
fn blend_factor_zero_returns_a() {
    assert_eq!(blend((100, 200, 50), (0, 0, 0), 0.0), (100, 200, 50));
}

#[test]
fn blend_factor_one_returns_b() {
    assert_eq!(blend((100, 200, 50), (0, 0, 0), 1.0), (0, 0, 0));
}

#[test]
fn blend_factor_half_returns_midpoint() {
    assert_eq!(blend((0, 0, 0), (100, 200, 50), 0.5), (50, 100, 25));
}

#[test]
fn blend_clamps_to_valid_range() {
    // Even with extreme values, result should be valid u8
    let result = blend((255, 255, 255), (255, 255, 255), 0.5);
    assert_eq!(result, (255, 255, 255));
}

// ── get_color ──

#[test]
fn get_color_found() {
    let mut sections = HashMap::new();
    let mut window = HashMap::new();
    window.insert("BackgroundNormal".to_owned(), "255,0,128".to_owned());
    sections.insert("Colors:Window".to_owned(), window);

    assert_eq!(get_color(&sections, "Colors:Window", "BackgroundNormal"), Some((255, 0, 128)));
}

#[test]
fn get_color_missing_section() {
    let sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    assert_eq!(get_color(&sections, "Colors:Window", "BackgroundNormal"), None);
}

#[test]
fn get_color_missing_key() {
    let mut sections = HashMap::new();
    sections.insert("Colors:Window".to_owned(), HashMap::new());
    assert_eq!(get_color(&sections, "Colors:Window", "BackgroundNormal"), None);
}

#[test]
fn get_color_invalid_rgb_value() {
    let mut sections = HashMap::new();
    let mut window = HashMap::new();
    window.insert("BackgroundNormal".to_owned(), "not,a,color".to_owned());
    sections.insert("Colors:Window".to_owned(), window);

    assert_eq!(get_color(&sections, "Colors:Window", "BackgroundNormal"), None);
}

#[test]
fn get_color_overflow_value() {
    let mut sections = HashMap::new();
    let mut window = HashMap::new();
    window.insert("BackgroundNormal".to_owned(), "256,0,0".to_owned());
    sections.insert("Colors:Window".to_owned(), window);

    assert_eq!(get_color(&sections, "Colors:Window", "BackgroundNormal"), None);
}

// ── kde_palette_from_sections ──

/// Build a `sections` fixture from `(section, key, value)` triples.
fn build_sections(rows: &[(&str, &str, &str)]) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (section, key, value) in rows {
        out.entry((*section).to_owned())
            .or_default()
            .insert((*key).to_owned(), (*value).to_owned());
    }
    out
}

/// Verbatim values from the user's `~/.config/kdeglobals`
/// (Catppuccin Mocha Sapphire Plasma colour scheme). The kdeglobals
/// encodes Catppuccin Mocha's palette by role, so a correct
/// KDE→slot mapping must reproduce the static `themes::catppuccin::MOCHA`
/// hex values byte-for-byte. Each blended slot below was hand-computed
/// from the inputs and lands exactly on the Catppuccin target with no
/// rounding drift.
#[expect(
    clippy::expect_used,
    reason = "test fixture is fully populated; Some is invariant"
)]
#[test]
fn kde_palette_matches_catppuccin_mocha_sapphire() {
    let sections = build_sections(&[
        // [Colors:Window] — chrome (titlebar / sidebar zone)
        ("Colors:Window", "BackgroundNormal", "24, 24, 37"),
        ("Colors:Window", "BackgroundAlternate", "17, 17, 27"),
        // [Colors:View] — content area
        ("Colors:View", "BackgroundNormal", "30, 30, 46"),
        ("Colors:View", "BackgroundAlternate", "24, 24, 37"),
        ("Colors:View", "ForegroundNormal", "205, 214, 244"),
        ("Colors:View", "ForegroundInactive", "166, 173, 200"),
        ("Colors:View", "ForegroundNegative", "243, 139, 168"),
        ("Colors:View", "ForegroundPositive", "166, 227, 161"),
        ("Colors:View", "ForegroundNeutral", "249, 226, 175"),
        ("Colors:View", "DecorationFocus", "116, 199, 236"),
        // [Colors:Button] — surface0 of clickable controls
        ("Colors:Button", "BackgroundNormal", "49, 50, 68"),
        // [Colors:Header] — Catppuccin uses mantle here (matches sidebar)
        ("Colors:Header", "BackgroundNormal", "24, 24, 37"),
        // [Colors:Selection] — sapphire accent
        ("Colors:Selection", "BackgroundNormal", "116, 199, 236"),
        // [WM] / [ColorEffects:Inactive] — active + inactive titlebar
        ("WM", "activeBackground", "30, 30, 46"),
        ("WM", "inactiveBackground", "17, 17, 27"),
        ("ColorEffects:Inactive", "Color", "30, 30, 46"),
        ("ColorEffects:Inactive", "ColorAmount", "0.5"),
    ]);

    let p = kde_palette_from_sections(&sections).expect("palette built");
    let g = |key: &str| p.colors.get(key).map(String::as_str);

    // Chrome / content trio
    assert_eq!(g("base"), Some("#1e1e2e"), "base ← view_bg");
    assert_eq!(g("mantle"), Some("#181825"), "mantle ← header_bg");
    assert_eq!(g("crust"), Some("#11111b"), "crust ← window_bg_alt");

    // Surface ramp (button_bg blended toward text). Every ratio was
    // chosen so the result lands exactly on Catppuccin Mocha.
    assert_eq!(g("surface0"), Some("#313244"), "surface0 ← button_bg");
    assert_eq!(g("surface1"), Some("#45475a"), "surface1 ← blend(button_bg, text, 0.125)");
    assert_eq!(g("surface2"), Some("#585b70"), "surface2 ← blend(button_bg, text, 0.25)");
    assert_eq!(g("overlay0"), Some("#6c7086"), "overlay0 ← blend(button_bg, text, 0.375)");
    assert_eq!(g("overlay1"), Some("#7f849c"), "overlay1 ← blend(button_bg, text, 0.5)");
    assert_eq!(g("overlay2"), Some("#9399b2"), "overlay2 ← blend(button_bg, text, 0.625)");

    // Text trio
    assert_eq!(g("text"), Some("#cdd6f4"), "text ← ForegroundNormal");
    assert_eq!(g("subtext0"), Some("#a6adc8"), "subtext0 ← ForegroundInactive (direct)");
    assert_eq!(g("subtext1"), Some("#bac2de"), "subtext1 ← blend(text, ForegroundInactive, 0.5)");

    // Border == surface0 by design
    assert_eq!(g("border"), Some("#313244"), "border == surface0");

    // Accent + the semantic trio, which Plasma publishes as its three
    // [Colors:View] status foregrounds.
    assert_eq!(p.accent, "#74c7ec", "accent ← Colors:Selection BackgroundNormal");
    assert_eq!(p.red, "#f38ba8", "red ← Colors:View ForegroundNegative");
    assert_eq!(p.green, "#a6e3a1", "green ← Colors:View ForegroundPositive");
    assert_eq!(p.yellow, "#f9e2af", "yellow ← Colors:View ForegroundNeutral");
}

/// A scheme that omits the status foregrounds has to land on Breeze's own
/// defaults, not on a neutral — the macOS-style traffic lights, the success /
/// warning toasts and the star rating all read `green` / `yellow`.
#[expect(
    clippy::expect_used,
    reason = "test fixture is fully populated; Some is invariant"
)]
#[test]
fn kde_palette_semantics_fall_back_to_breeze_defaults() {
    let sections = build_sections(&[
        ("Colors:Window", "BackgroundNormal", "24, 24, 37"),
        ("Colors:View", "BackgroundNormal", "30, 30, 46"),
        ("Colors:View", "ForegroundNormal", "205, 214, 244"),
        ("Colors:Selection", "BackgroundNormal", "116, 199, 236"),
    ]);

    let p = kde_palette_from_sections(&sections).expect("palette built");
    assert_eq!(p.red, "#da4453", "red ← Breeze ForegroundNegative default");
    assert_eq!(p.green, "#27ae60", "green ← Breeze ForegroundPositive default");
    assert_eq!(p.yellow, "#f67400", "yellow ← Breeze ForegroundNeutral default");
}

#[expect(
    clippy::expect_used,
    reason = "test fixture is fully populated; Some is invariant"
)]
#[test]
fn kde_palette_falls_back_when_button_section_missing() {
    // Same scheme minus [Colors:Button] — surface0 should fall back
    // to window_bg, and the surface ramp should still derive coherently.
    let sections = build_sections(&[
        ("Colors:Window", "BackgroundNormal", "24, 24, 37"),
        ("Colors:Window", "BackgroundAlternate", "17, 17, 27"),
        ("Colors:View", "BackgroundNormal", "30, 30, 46"),
        ("Colors:View", "ForegroundNormal", "205, 214, 244"),
        ("Colors:View", "ForegroundInactive", "166, 173, 200"),
        ("Colors:Header", "BackgroundNormal", "24, 24, 37"),
        ("Colors:Selection", "BackgroundNormal", "116, 199, 236"),
        ("WM", "activeBackground", "30, 30, 46"),
    ]);

    let p = kde_palette_from_sections(&sections).expect("palette built");
    // surface0 falls back to window_bg = (24,24,37) = #181825
    assert_eq!(p.colors.get("surface0").map(String::as_str), Some("#181825"));
    // border tracks surface0 even on fallback
    assert_eq!(p.colors.get("border").map(String::as_str), Some("#181825"));
}

#[test]
fn kde_palette_returns_none_when_required_sections_missing() {
    // Missing [Colors:Window] BackgroundNormal — must reject, not panic.
    let sections = build_sections(&[("Colors:View", "BackgroundNormal", "30, 30, 46")]);
    assert!(kde_palette_from_sections(&sections).is_none());
}
