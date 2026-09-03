//! The Motion card's one persisted setting.

use crate::services;
use crate::state::AppState;
use melodia_core::error::AppError;

/// Persist the Skip Startup Animation switch.
///
/// A pure disk write: nothing applies it in the running session, since the only mount it
/// speaks for is the one `boot::ui_setup` sets up before the window is shown.
pub fn set_skip_startup_animation(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.motion.skip_startup_animation = on;
    })
}
