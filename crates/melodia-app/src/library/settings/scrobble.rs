//! Scrobbling-section setters.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

pub fn set_scrobble_lastfm_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.scrobble.lastfm_enabled = enabled;
    })
}

pub fn set_scrobble_listenbrainz_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.scrobble.listenbrainz_enabled = enabled;
    })
}

/// The love↔favorite sync.
pub fn set_scrobble_lastfm_love_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.scrobble.lastfm_love_enabled = enabled;
    })
}

/// The love↔favorite sync.
pub fn set_scrobble_listenbrainz_love_enabled(
    paths: &Paths,
    enabled: bool,
) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.scrobble.listenbrainz_love_enabled = enabled;
    })
}
