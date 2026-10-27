//! Whether Windows draws light or dark, two answers under Settings ▸ Personalization ▸ Colors: the
//! app mode a theme's System variant follows, and the Windows mode the taskbar's tray icons sit
//! on. **Fails light**, the mode Windows draws in until the user picks another.

use super::registry::read_user_dword;

const PERSONALIZE_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

/// `"light"` or `"dark"`, `SystemColorState::theme`'s vocabulary.
pub fn app_theme() -> &'static str {
    theme_for(read_user_dword(PERSONALIZE_KEY, "AppsUseLightTheme"))
}

pub fn taskbar_theme() -> &'static str {
    theme_for(read_user_dword(PERSONALIZE_KEY, "SystemUsesLightTheme"))
}

/// The decision apart from the registry: dark only while the value says so outright.
fn theme_for(uses_light_theme: Option<u32>) -> &'static str {
    if uses_light_theme == Some(0) { "dark" } else { "light" }
}

#[cfg(test)]
#[path = "tests/color_mode_tests.rs"]
mod tests;
