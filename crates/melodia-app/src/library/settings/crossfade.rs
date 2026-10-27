//! Crossfade-section setters.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_playback::player::playback::crossfade::clamp_crossfade_ms;

pub fn set_crossfade_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.crossfade.crossfade_enabled = enabled;
    })
}

/// Clamped, so a malformed write can't reach `settings.json`.
pub fn set_crossfade_duration_ms(paths: &Paths, ms: u32) -> Result<(), AppError> {
    let ms = clamp_crossfade_ms(ms);
    services::settings::mutate_settings(paths, move |settings| {
        settings.crossfade.crossfade_duration_ms = ms;
    })
}

/// Whether next, previous or picking a track also crossfades rather than cutting over.
pub fn set_crossfade_manual(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.crossfade.crossfade_manual = on;
    })
}

pub fn set_crossfade_skip_same_album(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.crossfade.crossfade_skip_same_album = on;
    })
}

/// Covers a user stop too, and the fade back in on resume.
pub fn set_crossfade_fade_on_pause(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.crossfade.crossfade_fade_on_pause = on;
    })
}
