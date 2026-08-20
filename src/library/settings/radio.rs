//! The Radio section's master switch. Persists to `settings.json`; the in-memory
//! shadow on [`AppState`] is refreshed separately by the UI callback once the
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
