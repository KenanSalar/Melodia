//! Whether Windows draws light or dark, under Settings ▸ Personalization ▸ Colors. It keeps two
//! answers: the default app mode, which a theme's System variant follows, and the Windows mode,
//! which paints the taskbar the tray icon sits on.
//!
//! **Fails light**: a value that won't read answers light, the mode Windows draws in until the
//! user picks another.

use super::registry::read_user_dword;

const PERSONALIZE_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

/// The default app mode, as `"light"` or `"dark"`, the vocabulary `SystemColorState::theme` is
/// written in.
pub fn app_theme() -> &'static str {
    theme_for(read_user_dword(PERSONALIZE_KEY, "AppsUseLightTheme"))
}

/// The Windows mode, as `"light"` or `"dark"`: what the taskbar and its tray icons sit on.
pub fn taskbar_theme() -> &'static str {
    theme_for(read_user_dword(PERSONALIZE_KEY, "SystemUsesLightTheme"))
}

/// The decision, apart from the registry: dark only while the value says so outright.
fn theme_for(uses_light_theme: Option<u32>) -> &'static str {
    if uses_light_theme == Some(0) { "dark" } else { "light" }
}

#[cfg(test)]
#[path = "tests/color_mode_tests.rs"]
mod tests;
