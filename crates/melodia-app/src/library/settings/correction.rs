//! Headphone correction setters.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::entities::correction::DeviceCorrections;
use melodia_core::error::AppError;

pub fn set_correction_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.correction.correction_enabled = enabled;
    })
}

/// Every device's correction at once, the map being one field of `eq.json`.
pub fn set_corrections(paths: &Paths, corrections: DeviceCorrections) -> Result<(), AppError> {
    services::settings::mutate_eq(paths, move |eq| eq.corrections = corrections)
}

pub fn set_correction_online(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.correction.correction_online = enabled;
    })
}
