//! The Radio section's switches. Persist to `settings.json`; the in-memory
//! shadows on [`AppState`] are refreshed separately by the UI callbacks once the
//! write commits, the same kick-after-persist ordering [`super::discord`] uses.

use crate::error::AppError;
use crate::services;
use crate::state::AppState;

/// Persist the Radio master toggle. Off by default (opt-in).
pub fn set_radio_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.radio.radio_enabled = enabled;
    })
}

/// Persist whether directory results hide segmented stations.
pub fn set_radio_hide_hls(state: &AppState, hide: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.radio.radio_hide_hls = hide;
    })
}

/// Persist whether playing a station reports a click back to the directory.
pub fn set_radio_send_clicks(state: &AppState, send: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.radio.radio_send_clicks = send;
    })
}
