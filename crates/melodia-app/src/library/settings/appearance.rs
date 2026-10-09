//! Appearance-section setters (theme / variant / accent, dynamic colour
//! style, match-unfocused, corner radius). Every *setter* persists through
//! [`crate::services::settings::mutate_settings`] so a burst of clicks
//! can't race over the read-mutate-write window.

use crate::services::{self, settings::ThemePreference};
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Backfill `theme_preferences[theme_id]` from the top-level theme fields when
/// the active theme has no entry yet, persisting only when one was missing.
///
/// A boot-time migration for `settings.json` written before that map existed:
/// without it, a user who launches with a custom accent (say
/// `catppuccin/mocha/yellow`) loses it on the first theme swap, the lookup
/// missing and falling back to defaults. Distinct from [`set_appearance`],
/// which records a pick the user just made — this writes what an earlier build
/// never did, and no-ops once it has.
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

/// Persist the user's appearance picks (theme / variant / accent) into
/// `settings.json`. Updates the three top-level fields *and*
/// `theme_preferences[theme_id]` so each theme remembers its last
/// variant + accent across switches (Tauri's per-theme memory). The
/// read-mutate-write window is serialized by `mutate_settings`, so a
/// burst of accent / variant clicks can't lose updates. A Material You pick
/// keeps the theme's last real accent, as [`ThemePreference::new`] argues.
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

/// Persist the Material 3 dynamic-colour style ("none" / `tonal_spot` /
/// `vibrant` / …). Drives the Material You generator in
/// `tasks::material_you`. Setting "none" disables dynamic colour and
/// restores the static M3 palette.
pub fn set_dynamic_color_style(paths: &Paths, style: String) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.dynamic_color_style = style;
    })
}

/// Persist the user toggle for "Match Unfocused Window Background".
/// The runtime gate (focus event → `Theme.window-focused` write) lives
/// in `melodia-views`' `ui/window_chrome/`'s winit filter; this helper only commits
/// the new value to disk so the next process boot picks it up.
pub fn set_match_unfocused_to_system_bg(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.layout.match_unfocused_to_system_bg = on;
    })
}

/// Persist the user's window corner radius (logical pixels). Clamped to
/// `0..=MAX_CORNER_RADIUS` so a malformed UI write can't push an
/// out-of-range value into `settings.json`. The runtime application
/// (mirroring the value into `Theme.shell-radius`) happens synchronously
/// in `melodia-views`' `ui/appearance/`'s `wire_corner_radius_changed` *before* this
/// async persist, so the UI repaints immediately even when the disk
/// write hasn't completed.
pub fn set_corner_radius(paths: &Paths, px: u32) -> Result<(), AppError> {
    let clamped = px.min(crate::services::settings::MAX_CORNER_RADIUS);
    services::settings::mutate_settings(paths, move |settings| {
        settings.corner_radius = clamped;
    })
}

#[cfg(test)]
#[path = "tests/appearance_tests.rs"]
mod tests;
