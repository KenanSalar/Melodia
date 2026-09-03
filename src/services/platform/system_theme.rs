use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::watch;
use zbus::MatchRule;

use crate::themes::kde::KdeColorPalette;

/// Color scheme values from the XDG Desktop Portal specification.
/// See: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Settings.html>
const COLOR_SCHEME_PREFER_LIGHT: u32 = 2;
// 0 = no preference (treated as dark), 1 = prefer dark, 2 = prefer light

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.Settings";
const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME_KEY: &str = "color-scheme";

pub(crate) fn color_scheme_to_str(value: u32) -> &'static str {
    if value == COLOR_SCHEME_PREFER_LIGHT {
        "light"
    } else {
        "dark"
    }
}

fn unwrap_variant_u32(value: zbus::zvariant::Value<'_>) -> Option<u32> {
    use zbus::zvariant::Value;
    let mut current = value;
    loop {
        match current {
            Value::Value(inner) => current = *inner,
            Value::U32(v) => return Some(v),
            _ => return None,
        }
    }
}

/// Extract color-scheme u32 from a D-Bus reply body containing nested variants.
/// Handles both `ReadOne` (single variant) and deprecated `Read` (double variant).
fn extract_color_scheme(reply: &zbus::Message) -> Option<u32> {
    use zbus::zvariant::Value;

    let body = reply.body();
    let outer: Value = body.deserialize().ok()?;
    unwrap_variant_u32(outer)
}

/// Query the XDG Desktop Portal for the current system color scheme.
/// Returns "dark" or "light".
///
/// Runs the blocking zbus call inside `spawn_blocking` so it never enters the
/// tokio reactor (the project keeps zbus's tokio feature off — see CLAUDE.md).
pub async fn get_system_theme() -> String {
    tokio::task::spawn_blocking(query_portal_color_scheme_blocking)
        .await
        .ok()
        .flatten()
        .map(color_scheme_to_str)
        .map_or_else(|| "dark".to_owned(), str::to_owned)
}

/// Synchronous variant of [`get_system_theme`] for startup paths that run
/// before the Slint event loop and don't have a future to await on. Same
/// `"dark"` fallback when the portal is unreachable.
pub fn get_system_theme_blocking() -> String {
    query_portal_color_scheme_blocking()
        .map(color_scheme_to_str)
        .map_or_else(|| "dark".to_owned(), str::to_owned)
}

fn query_portal_color_scheme_blocking() -> Option<u32> {
    let conn = zbus::blocking::Connection::session().ok()?;

    let mut reply = conn
        .call_method(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            Some(PORTAL_IFACE),
            "ReadOne",
            &(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY),
        )
        .ok();

    if reply.is_none() {
        // ReadOne not available — try deprecated Read (portal v1)
        reply = conn
            .call_method(
                Some(PORTAL_BUS),
                PORTAL_PATH,
                Some(PORTAL_IFACE),
                "Read",
                &(APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY),
            )
            .ok();
    }

    extract_color_scheme(&reply?)
}

/// Spawn a background task that listens for XDG portal `SettingChanged`
/// signals and forwards a fresh [`crate::themes::SystemColorState`] on every appearance
/// change. The payload bundles the current dark/light theme *and* the
/// re-read KDE palette so the UI consumer doesn't have to coordinate two
/// separate channels — KDE's `kdeglobals` is the same file Plasma rewrites
/// when the colour scheme changes, so refreshing it on every portal tick
/// is correct and cheap (a few KB read on a desktop event).
///
/// Uses `zbus::blocking::*` inside `spawn_blocking` so it doesn't share an
/// async zbus connection with Slint's a11y subsystem (see CLAUDE.md zbus
/// footgun).
pub fn spawn_color_watcher(state_tx: watch::Sender<crate::themes::SystemColorState>) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = watch_color_changes(&state_tx) {
            log::warn!("System theme watcher stopped: {e}");
        }
    });
}

fn watch_color_changes(
    state_tx: &watch::Sender<crate::themes::SystemColorState>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let conn = zbus::blocking::Connection::session()?;

    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(PORTAL_BUS)?
        .path(PORTAL_PATH)?
        .interface(PORTAL_IFACE)?
        .member("SettingChanged")?
        .build();

    let iter = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(10))?;

    for msg in iter {
        let Ok(msg) = msg else {
            continue;
        };

        let body_result: Result<(String, String, zbus::zvariant::OwnedValue), _> =
            msg.body().deserialize();
        if let Ok((namespace, key, value)) = body_result
            && namespace == APPEARANCE_NAMESPACE
            && key == COLOR_SCHEME_KEY
        {
            let scheme = owned_value_to_u32(&value);
            let theme_str = color_scheme_to_str(scheme);
            log::info!("System theme changed: {theme_str}");
            // `send_modify` preserves `material_you` — the dynamic
            // Material 3 palette is owned by `tasks::material_you` and
            // must survive OS theme flips. The coordinator subscribes to
            // both the kick channel *and* `view_model_rx`, so when the
            // user's variant is "system" and the OS dark/light flips,
            // `appearance::spawn_os_state_watcher` triggers a kick and
            // the coordinator regenerates the dynamic palette for the
            // new `is_dark`. Until then, the previously generated
            // palette stays painted — that's a fine transient.
            state_tx.send_modify(|s| {
                theme_str.clone_into(&mut s.theme);
                s.kde_palette = get_kde_colors();
            });
        }
    }

    Ok(())
}

fn owned_value_to_u32(value: &zbus::zvariant::OwnedValue) -> u32 {
    if let Ok(v) = <u32 as TryFrom<&zbus::zvariant::OwnedValue>>::try_from(value) {
        return v;
    }
    if let Ok(v) = <zbus::zvariant::Value as TryFrom<&zbus::zvariant::OwnedValue>>::try_from(value)
    {
        return unwrap_variant_u32(v).unwrap_or(0);
    }
    0
}

// ---------------------------------------------------------------------------
// KDE color scheme reading from ~/.config/kdeglobals
// ---------------------------------------------------------------------------

fn kdeglobals_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")).join("kdeglobals")
}

/// Parse an "R,G,B" string into (u8, u8, u8).
pub(crate) fn parse_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let parts: Vec<&str> = s.trim().split(',').collect();
    if parts.len() != 3 {
        return None;
    }
    let r: u8 = parts[0].trim().parse().ok()?;
    let g: u8 = parts[1].trim().parse().ok()?;
    let b: u8 = parts[2].trim().parse().ok()?;
    Some((r, g, b))
}

pub(crate) fn rgb_to_hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Blend two RGB colors by a factor (0.0 = a, 1.0 = b).
pub(crate) fn blend(a: (u8, u8, u8), b: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let mix = |a: u8, b: u8| -> u8 {
        let v = f32::from(a) * (1.0 - factor) + f32::from(b) * factor;
        // v is the result of two non-negative weighted u8 inputs (each ≤ 255)
        // and a clamp to [0, 255]; the f32→u8 narrowing is well-defined.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "v is clamped to [0, 255] before narrowing"
        )]
        let mixed = v.round().clamp(0.0, 255.0) as u8;
        mixed
    };
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

fn read_kdeglobals() -> Option<HashMap<String, HashMap<String, String>>> {
    let path = kdeglobals_path();
    let content = std::fs::read_to_string(&path).ok()?;

    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            line[1..line.len() - 1].clone_into(&mut current_section);
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            sections
                .entry(current_section.clone())
                .or_default()
                .insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }

    Some(sections)
}

fn get_color(
    sections: &HashMap<String, HashMap<String, String>>,
    section: &str,
    key: &str,
) -> Option<(u8, u8, u8)> {
    sections.get(section)?.get(key).and_then(|v| parse_rgb(v))
}

/// Parse `[ColorEffects:Inactive]` into `(tint_color, color_amount)`.
/// Returns `None` if the section is missing or its `Color` /
/// `ColorAmount` keys are unparseable. `ColorAmount` is clamped to
/// `[0.0, 1.0]` so a malformed value can't produce nonsense blends.
fn get_inactive_color_effect(
    sections: &HashMap<String, HashMap<String, String>>,
) -> Option<((u8, u8, u8), f32)> {
    let section = sections.get("ColorEffects:Inactive")?;
    let tint = parse_rgb(section.get("Color")?)?;
    let amount: f32 = section.get("ColorAmount")?.trim().parse().ok()?;
    Some((tint, amount.clamp(0.0, 1.0)))
}

/// Read the KDE color scheme from `~/.config/kdeglobals` and map it to our theme slots.
pub fn get_kde_colors() -> Option<KdeColorPalette> {
    let sections = read_kdeglobals()?;
    kde_palette_from_sections(&sections)
}

/// Map a parsed kdeglobals section tree to a `KdeColorPalette`. Split out
/// from `get_kde_colors()` so unit tests can verify the slot mapping
/// against an in-memory fixture without touching the user's config.
///
/// **Source mental model:** KDE's colour groups are role-based, not
/// brightness-based.
/// * `[Colors:Window]` is **chrome** — titlebar / sidebar / dock zone.
/// * `[Colors:View]`   is **content area** — where lists and text live.
/// * `[Colors:Button]` is the **surface0** of clickable controls.
///
/// We feed those into Catppuccin slots as:
/// * `base   ← view_bg`     (content area)
/// * `mantle ← header_bg`   (chrome — falls back to `[WM] activeBackground` then `window_bg`)
/// * `crust  ← window_bg_alt` (deepest chrome layer)
///
/// The intermediate surface ramp (`surface1/2`, `overlay0..2`) is then
/// reconstructed by linearly blending `button_bg` toward `text` at
/// fixed Catppuccin-matched ratios. Verified on Catppuccin Mocha
/// Sapphire to land within one byte per channel of the static palette;
/// degrades coherently on other Plasma schemes because `text` is
/// always the maximum-contrast colour against the surface ramp.
pub(crate) fn kde_palette_from_sections(
    sections: &HashMap<String, HashMap<String, String>>,
) -> Option<KdeColorPalette> {
    let window_bg = get_color(sections, "Colors:Window", "BackgroundNormal")?;
    let view_bg = get_color(sections, "Colors:View", "BackgroundNormal")?;
    // Window/alt is the deepest chrome shade in every Plasma scheme
    // we've checked — exactly what Catppuccin calls `crust`. Fall back
    // to a 30% darken of `window_bg` when the scheme omits the alt.
    let window_bg_alt = get_color(sections, "Colors:Window", "BackgroundAlternate")
        .unwrap_or_else(|| blend(window_bg, (0, 0, 0), 0.3));
    let button_bg = get_color(sections, "Colors:Button", "BackgroundNormal").unwrap_or(window_bg);
    let header_bg = get_color(sections, "Colors:Header", "BackgroundNormal")
        .or_else(|| get_color(sections, "WM", "activeBackground"))
        .unwrap_or(window_bg);
    // Modern KDE Plasma (6.x) computes the inactive titlebar by
    // applying `[ColorEffects:Inactive]` to the *active titlebar*
    // colour — which Breeze sources from `[WM] activeBackground`,
    // NOT from `[Colors:Header] BackgroundNormal` (the latter drives
    // QML headers inside apps and frequently disagrees with the
    // window decoration's own palette). `header_bg` lands in our
    // `mantle` slot, but matching the OS titlebar requires the
    // `[WM]` source. `[WM] inactiveBackground` is mostly legacy and
    // frequently points at `crust` (way too dark), so we prefer the
    // computed blend and only fall back to it when the effects group
    // is missing entirely. Final fallback blends `wm_active` toward
    // `window_bg` for colour schemes with neither piece.
    //
    // ColorEffect=3 in Breeze is technically an HCY-luma-preserving
    // tint, but a linear RGB `blend(active, tint, amount)` matches
    // the painted output to within a pixel for every preset we've
    // checked.
    let wm_active = get_color(sections, "WM", "activeBackground").unwrap_or(header_bg);
    let inactive_titlebar = get_inactive_color_effect(sections)
        .map(|(tint, amount)| blend(wm_active, tint, amount))
        .or_else(|| get_color(sections, "WM", "inactiveBackground"))
        .unwrap_or_else(|| blend(wm_active, window_bg, 0.5));

    let text = get_color(sections, "Colors:View", "ForegroundNormal")?;
    let text_inactive = get_color(sections, "Colors:View", "ForegroundInactive")
        .unwrap_or_else(|| blend(text, window_bg, 0.5));

    let accent = get_color(sections, "Colors:Selection", "BackgroundNormal")
        .or_else(|| get_color(sections, "Colors:View", "DecorationFocus"))
        .unwrap_or((61, 174, 233));
    // Plasma's three status foregrounds map 1:1 onto our semantic slots.
    // Fallbacks are Breeze's own defaults, matching the static `themes::kde`
    // palette so a scheme that omits them lands where the Dark/Light variants
    // already sit rather than on a grey.
    let red = get_color(sections, "Colors:View", "ForegroundNegative").unwrap_or((218, 68, 83));
    let green = get_color(sections, "Colors:View", "ForegroundPositive").unwrap_or((39, 174, 96));
    let yellow = get_color(sections, "Colors:View", "ForegroundNeutral").unwrap_or((246, 116, 0));

    // Surface ramp from `button_bg` (= surface0 in Catppuccin) toward
    // `text` at the ratios that match Catppuccin Mocha exactly when
    // fed Catppuccin-encoded kdeglobals.
    let surface1 = blend(button_bg, text, 0.125);
    let surface2 = blend(button_bg, text, 0.25);
    let overlay0 = blend(button_bg, text, 0.375);
    let overlay1 = blend(button_bg, text, 0.5);
    let overlay2 = blend(button_bg, text, 0.625);
    let subtext1 = blend(text, text_inactive, 0.5);

    let mut colors = HashMap::new();
    colors.insert("base".into(), rgb_to_hex(view_bg.0, view_bg.1, view_bg.2));
    colors.insert("mantle".into(), rgb_to_hex(header_bg.0, header_bg.1, header_bg.2));
    colors.insert(
        "mantle_unfocused".into(),
        rgb_to_hex(inactive_titlebar.0, inactive_titlebar.1, inactive_titlebar.2),
    );
    colors.insert("crust".into(), rgb_to_hex(window_bg_alt.0, window_bg_alt.1, window_bg_alt.2));
    colors.insert("surface0".into(), rgb_to_hex(button_bg.0, button_bg.1, button_bg.2));
    colors.insert("surface1".into(), rgb_to_hex(surface1.0, surface1.1, surface1.2));
    colors.insert("surface2".into(), rgb_to_hex(surface2.0, surface2.1, surface2.2));
    colors.insert("overlay0".into(), rgb_to_hex(overlay0.0, overlay0.1, overlay0.2));
    colors.insert("overlay1".into(), rgb_to_hex(overlay1.0, overlay1.1, overlay1.2));
    colors.insert("overlay2".into(), rgb_to_hex(overlay2.0, overlay2.1, overlay2.2));
    colors.insert("text".into(), rgb_to_hex(text.0, text.1, text.2));
    colors.insert("subtext0".into(), rgb_to_hex(text_inactive.0, text_inactive.1, text_inactive.2));
    colors.insert("subtext1".into(), rgb_to_hex(subtext1.0, subtext1.1, subtext1.2));
    // `border == surface0` matches Catppuccin's intent — one fewer
    // free parameter, one fewer place to drift on non-Catppuccin schemes.
    colors.insert("border".into(), rgb_to_hex(button_bg.0, button_bg.1, button_bg.2));

    Some(KdeColorPalette {
        colors,
        accent: rgb_to_hex(accent.0, accent.1, accent.2),
        red: rgb_to_hex(red.0, red.1, red.2),
        green: rgb_to_hex(green.0, green.1, green.2),
        yellow: rgb_to_hex(yellow.0, yellow.1, yellow.2),
    })
}

#[cfg(test)]
#[path = "tests/system_theme_tests.rs"]
mod tests;
