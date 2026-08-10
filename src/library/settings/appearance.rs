//! Appearance-section setters (theme / variant / accent, dynamic colour
//! style, match-unfocused, corner radius). All persist through
//! [`crate::services::settings::mutate_settings`] so a burst of clicks
//! can't race over the read-mutate-write window.

use crate::error::AppError;
use crate::services::{
    self,
    settings::{SettingsData, ThemePreference},
};
use crate::state::AppState;

/// Backfill `theme_preferences[theme_id]` from the top-level theme fields when
/// the active theme has no entry yet, persisting only when one was missing.
///
/// A boot-time migration for `settings.json` written before that map existed:
/// without it, a user who launches with a custom accent (say
/// `catppuccin/mocha/yellow`) loses it on the first theme swap, the lookup
/// missing and falling back to defaults. Distinct from [`set_appearance`],
/// which records a pick the user just made — this writes what an earlier build
/// never did, and no-ops once it has.
///
/// Takes the settings the caller already read rather than re-reading them:
/// `mutate_settings` writes unconditionally, which would turn a one-off
/// migration into a full rewrite of `settings.json` on every launch.
pub fn seed_theme_preference(
    state: &AppState,
    mut settings: SettingsData,
) -> Result<(), AppError> {
    if settings.theme_preferences.contains_key(&settings.theme_id) {
        return Ok(());
    }
    let last_static_accent = if settings.accent_color == crate::themes::MATERIAL_YOU_ACCENT_ID {
        None
    } else {
        Some(settings.accent_color.clone())
    };
    settings.theme_preferences.insert(
        settings.theme_id.clone(),
        ThemePreference {
            variant: settings.theme_variant.clone(),
            accent: settings.accent_color.clone(),
            last_static_accent,
        },
    );
    services::settings::write_settings(&state.paths, &settings)
}

/// Persist the user's appearance picks (theme / variant / accent) into
/// `settings.json`. Updates the three top-level fields *and*
/// `theme_preferences[theme_id]` so each theme remembers its last
/// variant + accent across switches (Tauri's per-theme memory). The
/// read-mutate-write window is serialized by `mutate_settings`, so a
/// burst of accent / variant clicks can't lose updates.
///
/// `last_static_accent` is updated only when `accent_color` is *not*
/// `MATERIAL_YOU_ACCENT_ID` — Material You picks shouldn't overwrite
/// the user's last real accent, so disabling Color Style (or losing
/// artwork) can fall back to it.
pub fn set_appearance(
    state: &AppState,
    theme_id: String,
    theme_variant: String,
    accent_color: String,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        let preserved_static = settings
            .theme_preferences
            .get(&theme_id)
            .and_then(|p| p.last_static_accent.clone());
        let last_static_accent = if accent_color == crate::themes::MATERIAL_YOU_ACCENT_ID {
            preserved_static
        } else {
            Some(accent_color.clone())
        };
        settings.theme_preferences.insert(
            theme_id.clone(),
            ThemePreference {
                variant: theme_variant.clone(),
                accent: accent_color.clone(),
                last_static_accent,
            },
        );
        settings.theme_id = theme_id;
        settings.theme_variant = theme_variant;
        settings.accent_color = accent_color;
    })
}

/// Persist the Material 3 dynamic-colour style ("none" / `tonal_spot` /
/// `vibrant` / …). Drives the Material You generator in
/// `tasks::material_you`. Setting "none" disables dynamic colour and
/// restores the static M3 palette.
pub fn set_dynamic_color_style(state: &AppState, style: String) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.dynamic_color_style = style;
    })
}

/// Persist the user toggle for "Match Unfocused Window Background".
/// The runtime gate (focus event → `Theme.window-focused` write) lives
/// in `src/ui/window_chrome/`'s winit filter; this helper only commits
/// the new value to disk so the next process boot picks it up.
pub fn set_match_unfocused_to_system_bg(
    state: &AppState,
    on: bool,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.layout.match_unfocused_to_system_bg = on;
    })
}

/// Persist the user's window corner radius (logical pixels). Clamped to
/// `0..=MAX_CORNER_RADIUS` so a malformed UI write can't push an
/// out-of-range value into `settings.json`. The runtime application
/// (mirroring the value into `Theme.shell-radius`) happens synchronously
/// in `src/ui/appearance/`'s `wire_corner_radius_changed` *before* this
/// async persist, so the UI repaints immediately even when the disk
/// write hasn't completed.
pub fn set_corner_radius(state: &AppState, px: u32) -> Result<(), AppError> {
    let clamped = px.min(crate::services::settings::MAX_CORNER_RADIUS);
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.corner_radius = clamped;
    })
}
