//! The Diagnostics card's one persisted setting.

use crate::services;
use crate::state::AppState;
use melodia_core::error::AppError;

/// Persist the Verbose Logging switch.
///
/// A pure disk write — the caller has already applied the level live through
/// [`melodia_platform::services::platform::logging::set_verbose`]. What this buys is the next
/// launch, which `logging::install` starts at the same level.
pub fn set_verbose_logging(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.diagnostics.verbose_logging = on;
    })
}
