//! Equalizer-section setters.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_playback::player::playback::equalizer;

pub fn set_eq_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.equalizer.eq_enabled = enabled;
    })
}

/// Clamped, so a malformed write can't reach `settings.json`.
pub fn set_eq_preamp(paths: &Paths, preamp_db: f32) -> Result<(), AppError> {
    let preamp_db = equalizer::clamp_preamp(preamp_db);
    services::settings::mutate_settings(paths, move |settings| {
        settings.equalizer.eq_preamp = preamp_db;
    })
}

/// One write for both, since a drag release, a preset pick and a reset each change both. Gains
/// are clamped and length-normalised; the name is stored verbatim, hydration showing Custom for
/// one that isn't built in.
pub fn set_eq_band_gains_and_preset(
    paths: &Paths,
    gains: &[f32],
    preset: String,
) -> Result<(), AppError> {
    let norm = equalizer::normalize_gains(gains).to_vec();
    services::settings::mutate_settings(paths, move |settings| {
        settings.equalizer.eq_band_gains = norm;
        settings.equalizer.eq_selected_preset = preset;
    })
}
