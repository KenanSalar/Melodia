use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::watch;
use zbus::MatchRule;

use melodia_core::error::describe;
use melodia_core::themes::is_light_hex;
use melodia_core::themes::kde::KdeColorPalette;

/// The portal's `color-scheme`: 0 no preference (read as dark), 1 prefer dark, 2 prefer light.
const COLOR_SCHEME_PREFER_LIGHT: u32 = 2;

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_IFACE: &str = "org.freedesktop.portal.Settings";
const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME_KEY: &str = "color-scheme";

pub(crate) fn color_scheme_to_str(value: u32) -> &'static str {
    if value == COLOR_SCHEME_PREFER_LIGHT { "light" } else { "dark" }
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

/// Unwraps `ReadOne`'s single variant and the deprecated `Read`'s double one alike.
fn extract_color_scheme(reply: &zbus::Message) -> Option<u32> {
    use zbus::zvariant::Value;

    let body = reply.body();
    let outer: Value = body.deserialize().ok()?;
    unwrap_variant_u32(outer)
}

/// The portal's colour scheme as `"dark"` or `"light"`, dark when it is unreachable. Blocks on
/// D-Bus, so it belongs to startup before the event loop.
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
        // Portal v1 has only the deprecated `Read`.
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

/// Forwards a fresh [`melodia_core::themes::SystemColorState`] on every portal appearance change,
/// with `kdeglobals` re-read beside the theme so the consumer watches one channel: Plasma rewrites
/// that file on the same scheme change. `zbus::blocking` inside `spawn_blocking`, per the zbus
/// footgun.
pub fn spawn_color_watcher(state_tx: watch::Sender<melodia_core::themes::SystemColorState>) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = watch_color_changes(&state_tx) {
            log::warn!("System theme watcher stopped: {}", describe(&*e));
        }
    });
}

fn watch_color_changes(
    state_tx: &watch::Sender<melodia_core::themes::SystemColorState>,
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
            // `send_modify`, so the `material_you` palette `tasks::material_you` owns survives
            // the flip until that coordinator regenerates it for the new `is_dark`.
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

/// A `KConfig` file's groups, each mapping its keys to their raw values.
type KConfig = HashMap<String, HashMap<String, String>>;

fn kde_config_path(name: &str) -> Option<PathBuf> {
    Some(dirs::config_dir()?.join(name))
}

/// An `R,G,B` value.
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

/// `factor` 0.0 is `a`, 1.0 is `b`.
pub(crate) fn blend(a: (u8, u8, u8), b: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let mix = |a: u8, b: u8| -> u8 {
        let v = f32::from(a) * (1.0 - factor) + f32::from(b) * factor;
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

fn read_kconfig(path: &Path) -> Option<KConfig> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(parse_kdeglobals(&content))
}

/// Split from the read so a test can show the parser produces the section tree the mapping's
/// suites hand-build.
fn parse_kdeglobals(content: &str) -> KConfig {
    let mut sections = KConfig::new();
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

    sections
}

fn get_color(sections: &KConfig, section: &str, key: &str) -> Option<(u8, u8, u8)> {
    sections.get(section)?.get(key).and_then(|v| parse_rgb(v))
}

/// `[ColorEffects:Inactive]` as `(tint, amount)`, `None` where either key won't parse. The amount
/// is clamped to `[0.0, 1.0]`, `blend` extrapolating past both colours otherwise.
fn get_inactive_color_effect(sections: &KConfig) -> Option<((u8, u8, u8), f32)> {
    let section = sections.get("ColorEffects:Inactive")?;
    let tint = parse_rgb(section.get("Color")?)?;
    let amount: f32 = section.get("ColorAmount")?.trim().parse().ok()?;
    Some((tint, amount.clamp(0.0, 1.0)))
}

/// The KDE colour scheme in `~/.config/kdeglobals`, mapped onto our theme slots.
pub fn get_kde_colors() -> Option<KdeColorPalette> {
    let sections = read_kconfig(&kde_config_path("kdeglobals")?)?;
    kde_palette_from_sections(&sections)
}

/// The Plasma style a `plasmarc` naming none is on, and the one that ships no `colors` of its own.
const DEFAULT_PLASMA_STYLE: &str = "default";

/// Breeze Light's window background, which `KColorScheme` falls back to when no file names one.
const PLASMA_DEFAULT_WINDOW_BG: (u8, u8, u8) = (239, 240, 241);

/// Whether the Plasma panel paints light. Off the panel's own colours rather than the portal: the
/// panel follows the Plasma *style*, so Breeze Twilight puts a dark panel over a light app scheme.
/// A key the style's `colors` leaves out falls through to `kdeglobals`, as Plasma's config does.
/// **Fails light**, Breeze Light being what Plasma paints with no file at all.
pub fn plasma_panel_is_light() -> bool {
    let window_bg = [plasma_style_colors_path(), kde_config_path("kdeglobals")]
        .into_iter()
        .flatten()
        .find_map(|path| get_color(&read_kconfig(&path)?, "Colors:Window", "BackgroundNormal"))
        .unwrap_or(PLASMA_DEFAULT_WINDOW_BG);
    rgb_is_light(window_bg)
}

fn rgb_is_light((r, g, b): (u8, u8, u8)) -> bool {
    is_light_hex((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
}

/// The active Plasma style's `colors` file in `QStandardPaths`' order, `None` for a style shipping
/// none, the default among them.
fn plasma_style_colors_path() -> Option<PathBuf> {
    let style = kde_config_path("plasmarc")
        .and_then(|path| read_kconfig(&path))
        .and_then(|mut sections| sections.get_mut("Theme")?.remove("name"))
        .unwrap_or_else(|| DEFAULT_PLASMA_STYLE.to_owned());
    let relative = Path::new("plasma/desktoptheme").join(style).join("colors");
    xdg_data_dirs().into_iter().map(|dir| dir.join(&relative)).find(|path| path.is_file())
}

/// `XDG_DATA_HOME`, then `XDG_DATA_DIRS` or the spec's default. A relative entry is invalid per
/// the spec, and joined it would resolve against the working directory.
fn xdg_data_dirs() -> Vec<PathBuf> {
    let system = std::env::var_os("XDG_DATA_DIRS")
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs::data_dir()
        .into_iter()
        .chain(std::env::split_paths(&system))
        .filter(|dir| dir.is_absolute())
        .collect()
}

/// How far a light scheme's `surface0` sits from the view background toward the text: cards,
/// dialogs and tooltips paint that slot, and a light scheme gives them no colour of its own.
const LIGHT_SURFACE0_STEP: f32 = 0.09;

/// A parsed `kdeglobals` tree as a `KdeColorPalette`, split out so tests map a fixture rather than
/// the user's config.
///
/// KDE's groups are role-based rather than brightness-based: `[Colors:View]` is the content area
/// (`base`), `[Colors:Header]` the chrome (`mantle`), `[Colors:Window]`'s alternate the deepest
/// chrome (`crust`), `[Colors:Button]` the controls (`surface0` on a dark scheme). The rest of the
/// ramp blends `button_bg` toward `text` at Catppuccin's ratios, `text` being the maximum-contrast
/// colour against it on any scheme.
pub(crate) fn kde_palette_from_sections(sections: &KConfig) -> Option<KdeColorPalette> {
    let window_bg = get_color(sections, "Colors:Window", "BackgroundNormal")?;
    let view_bg = get_color(sections, "Colors:View", "BackgroundNormal")?;
    let window_bg_alt = get_color(sections, "Colors:Window", "BackgroundAlternate")
        .unwrap_or_else(|| blend(window_bg, (0, 0, 0), 0.3));
    let button_bg = get_color(sections, "Colors:Button", "BackgroundNormal").unwrap_or(window_bg);
    let header_bg = get_color(sections, "Colors:Header", "BackgroundNormal")
        .or_else(|| get_color(sections, "WM", "activeBackground"))
        .unwrap_or(window_bg);
    // KWin paints an inactive titlebar from `[Colors:Header][Inactive]` where the scheme ships it,
    // as every Breeze scheme does, and applies the colour effect only without it. `[WM]
    // inactiveBackground` often points at `crust`, far too dark, so the effect comes first; a
    // linear blend matches Breeze's luma-preserving tint to within a pixel.
    let wm_active = get_color(sections, "WM", "activeBackground").unwrap_or(header_bg);
    let inactive_titlebar = get_color(sections, "Colors:Header][Inactive", "BackgroundNormal")
        .or_else(|| {
            get_inactive_color_effect(sections).map(|(tint, amount)| blend(wm_active, tint, amount))
        })
        .or_else(|| get_color(sections, "WM", "inactiveBackground"))
        .unwrap_or_else(|| blend(wm_active, window_bg, 0.5));

    let text = get_color(sections, "Colors:View", "ForegroundNormal")?;
    let text_inactive = get_color(sections, "Colors:View", "ForegroundInactive")
        .unwrap_or_else(|| blend(text, window_bg, 0.5));

    let accent = get_color(sections, "Colors:Selection", "BackgroundNormal")
        .or_else(|| get_color(sections, "Colors:View", "DecorationFocus"))
        .unwrap_or((61, 174, 233));
    // Fallbacks are Breeze's defaults, matching the static `themes::kde` palette, so a scheme that
    // omits them lands where the Dark/Light variants sit rather than on a grey.
    let red = get_color(sections, "Colors:View", "ForegroundNegative").unwrap_or((218, 68, 83));
    let green = get_color(sections, "Colors:View", "ForegroundPositive").unwrap_or((39, 174, 96));
    let yellow = get_color(sections, "Colors:View", "ForegroundNeutral").unwrap_or((246, 116, 0));

    // A light scheme's buttons sit level with the view, so its Button colour would paint every
    // card in the page's own colour.
    let surface0 =
        if rgb_is_light(view_bg) { blend(view_bg, text, LIGHT_SURFACE0_STEP) } else { button_bg };

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
    colors.insert("surface0".into(), rgb_to_hex(surface0.0, surface0.1, surface0.2));
    colors.insert("surface1".into(), rgb_to_hex(surface1.0, surface1.1, surface1.2));
    colors.insert("surface2".into(), rgb_to_hex(surface2.0, surface2.1, surface2.2));
    colors.insert("overlay0".into(), rgb_to_hex(overlay0.0, overlay0.1, overlay0.2));
    colors.insert("overlay1".into(), rgb_to_hex(overlay1.0, overlay1.1, overlay1.2));
    colors.insert("overlay2".into(), rgb_to_hex(overlay2.0, overlay2.1, overlay2.2));
    colors.insert("text".into(), rgb_to_hex(text.0, text.1, text.2));
    colors.insert("subtext0".into(), rgb_to_hex(text_inactive.0, text_inactive.1, text_inactive.2));
    colors.insert("subtext1".into(), rgb_to_hex(subtext1.0, subtext1.1, subtext1.2));
    // As Catppuccin has it: one fewer free parameter to drift on other schemes.
    colors.insert("border".into(), rgb_to_hex(surface0.0, surface0.1, surface0.2));

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
