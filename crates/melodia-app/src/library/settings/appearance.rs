//! Appearance-section setters: theme, variant and accent, dynamic colour, the unfocused tint and
//! the corner radius.

use crate::services::{self, settings::ThemePreference};
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Backfill `theme_preferences[theme_id]` from the top-level fields when the active theme has no
/// entry, so a `settings.json` from before the map keeps its custom accent through the first theme
/// swap. Writes only when one was missing.
pub fn seed_theme_preference(paths: &Paths) -> Result<(), AppError> {
    services::settings::mutate_settings_if(paths, |settings| {
        if settings.theme_preferences.contains_key(&settings.theme_id) {
            return false;
        }
        let preference = ThemePreference::new(
            settings.theme_variant.clone(),
            settings.accent_color.clone(),
            None,
        );
        settings.theme_preferences.insert(settings.theme_id.clone(), preference);
        true
    })
}

/// Updates the three top-level fields and `theme_preferences[theme_id]`, so each theme remembers
/// its last variant and accent. A Material You pick keeps the theme's last real accent, as
/// [`ThemePreference::new`] argues.
pub fn set_appearance(
    paths: &Paths,
    theme_id: String,
    theme_variant: String,
    accent_color: String,
) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        let preserved_static = settings.last_static_accent(&theme_id).map(str::to_owned);
        let preference =
            ThemePreference::new(theme_variant.clone(), accent_color.clone(), preserved_static);
        settings.theme_preferences.insert(theme_id.clone(), preference);
        settings.theme_id = theme_id;
        settings.theme_variant = theme_variant;
        settings.accent_color = accent_color;
    })
}

/// `"none"` turns dynamic colour off and restores the static palette.
pub fn set_dynamic_color_style(paths: &Paths, style: String) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.dynamic_color_style = style;
    })
}

pub fn set_match_unfocused_to_system_bg(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.layout.match_unfocused_to_system_bg = on;
    })
}

/// In logical pixels, clamped so a malformed UI write can't reach `settings.json`.
pub fn set_corner_radius(paths: &Paths, px: u32) -> Result<(), AppError> {
    let clamped = px.min(crate::services::settings::MAX_CORNER_RADIUS);
    services::settings::mutate_settings(paths, move |settings| {
        settings.corner_radius = clamped;
    })
}

#[cfg(test)]
#[path = "tests/appearance_tests.rs"]
mod tests;
