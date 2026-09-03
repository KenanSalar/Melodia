//! Scrobbling-section setters. These persist the per-service enable toggles to
//! `settings.json`; the in-memory shadow on `ScrobbleService` is refreshed
//! separately by the UI callback once the write commits, mirroring the
//! kick-after-persist ordering the appearance/EQ setters use. Shaped like
//! [`super::replaygain`].

use crate::services;
use crate::state::AppState;
use melodia_core::error::AppError;

/// Persist the Last.fm scrobbling toggle. Defaults to `false` on first launch.
pub fn set_scrobble_lastfm_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.scrobble.lastfm_enabled = enabled;
    })
}

/// Persist the `ListenBrainz` scrobbling toggle. Defaults to `false` on first launch.
pub fn set_scrobble_listenbrainz_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.scrobble.listenbrainz_enabled = enabled;
    })
}

/// Persist the Last.fm love↔favorite sync toggle. Defaults to `false`.
pub fn set_scrobble_lastfm_love_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.scrobble.lastfm_love_enabled = enabled;
    })
}

/// Persist the `ListenBrainz` love↔favorite sync toggle. Defaults to `false`.
pub fn set_scrobble_listenbrainz_love_enabled(
    state: &AppState,
    enabled: bool,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.scrobble.listenbrainz_love_enabled = enabled;
    })
}

/// Persist the `MusicBrainz` auto-tag toggle. Defaults to `false` on first launch.
pub fn set_scrobble_mbid_auto_tag(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.scrobble.mbid_auto_tag = enabled;
    })
}
