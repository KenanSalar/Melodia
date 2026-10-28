//! `ReplayGain`-section setters.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_playback::player::playback::replaygain::{self, RgMode};

pub fn set_replaygain_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.replaygain.rg_enabled = enabled;
    })
}

pub fn set_replaygain_mode(paths: &Paths, mode: RgMode) -> Result<(), AppError> {
    let token = mode.as_settings_str().to_owned();
    services::settings::mutate_settings(paths, move |settings| {
        settings.replaygain.rg_mode = token;
    })
}

/// Clamped, so a malformed write can't reach `settings.json`.
pub fn set_replaygain_preamp(paths: &Paths, preamp_db: f32) -> Result<(), AppError> {
    let preamp_db = replaygain::clamp_rg_preamp(preamp_db);
    services::settings::mutate_settings(paths, move |settings| {
        settings.replaygain.rg_preamp = preamp_db;
    })
}

pub fn set_replaygain_prevent_clipping(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.replaygain.rg_prevent_clipping = on;
    })
}
