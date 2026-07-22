//! Scrobbling-section setters. These persist the per-service enable toggles to
//! `settings.json`; the in-memory shadow on `ScrobbleService` is refreshed
//! separately by the UI callback (Phase 3) once the write commits, mirroring the
//! kick-after-persist ordering the appearance/EQ setters use. Shaped like
//! [`super::replaygain`].

use crate::error::AppError;
use crate::services;
use crate::state::AppState;

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

/// Persist the love↔favorite sync toggle. Defaults to `false` on first launch.
pub fn set_scrobble_love_sync_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.scrobble.love_sync_enabled = enabled;
    })
}

/// Persist the `MusicBrainz` auto-tag toggle. Defaults to `false` on first launch.
pub fn set_scrobble_mbid_auto_tag(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.scrobble.mbid_auto_tag = enabled;
    })
}
