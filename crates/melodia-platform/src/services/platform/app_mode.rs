//! Whether Windows asks apps to draw light or dark: the default app mode under Settings ▸
//! Personalization ▸ Colors, and what a theme's System variant follows on Windows.
//!
//! **Fails light**: a value that won't read answers light, the mode Windows draws apps in until the
//! user picks another.

use super::registry::read_user_dword;

const PERSONALIZE_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

/// Returns `"light"` or `"dark"`, the vocabulary `SystemColorState::theme` is written in.
pub fn system_theme() -> &'static str {
    theme_for(read_user_dword(PERSONALIZE_KEY, "AppsUseLightTheme"))
}

/// The decision, apart from the registry: dark only while the value says so outright.
fn theme_for(apps_use_light_theme: Option<u32>) -> &'static str {
    if apps_use_light_theme == Some(0) { "dark" } else { "light" }
}

#[cfg(test)]
#[path = "tests/app_mode_tests.rs"]
mod tests;
