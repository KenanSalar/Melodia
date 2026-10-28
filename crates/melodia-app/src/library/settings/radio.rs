//! The Radio section's switches.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

pub fn set_radio_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.radio.radio_enabled = enabled;
    })
}

/// Whether directory results hide segmented stations.
pub fn set_radio_hide_segmented(paths: &Paths, hide: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.radio.radio_hide_segmented = hide;
    })
}

/// Whether playing a station reports a click back to the directory.
pub fn set_radio_send_clicks(paths: &Paths, send: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.radio.radio_send_clicks = send;
    })
}

/// Whether songs heard on a station are scrobbled.
pub fn set_radio_scrobble(paths: &Paths, scrobble: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.radio.radio_scrobble = scrobble;
    })
}
