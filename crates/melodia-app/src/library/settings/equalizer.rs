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

/// Everything the equalizer's dropdown, bands and preamp say at once, as a preset pick writes it
/// and a reset takes it away.
#[derive(Debug, Clone, Copy)]
pub struct EqCurve {
    pub gains: [f32; equalizer::NUM_BANDS],
    pub preset: &'static str,
    pub preamp_db: f32,
}

impl EqCurve {
    pub fn flat() -> Self {
        Self {
            gains: [0.0; equalizer::NUM_BANDS],
            preset: equalizer::DEFAULT_PRESET,
            preamp_db: 0.0,
        }
    }

    /// Whatever the dropdown reads: bands dragged back to zero show Custom and sound flat.
    pub fn is_flat(&self) -> bool {
        self.preamp_db == 0.0 && self.gains.iter().all(|&db| db == 0.0)
    }
}

/// One write for the whole curve. Clamped, as the setters above are.
pub fn set_eq_curve(paths: &Paths, curve: &EqCurve) -> Result<(), AppError> {
    let gains = equalizer::normalize_gains(&curve.gains).to_vec();
    let preset = curve.preset.to_owned();
    let preamp_db = equalizer::clamp_preamp(curve.preamp_db);
    services::settings::mutate_settings(paths, move |settings| {
        settings.equalizer.eq_band_gains = gains;
        settings.equalizer.eq_selected_preset = preset;
        settings.equalizer.eq_preamp = preamp_db;
    })
}

/// A drag release's write, which leaves the preamp alone. Gains are clamped and length-normalised;
/// the name is stored verbatim, hydration showing Custom for one that isn't built in.
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
