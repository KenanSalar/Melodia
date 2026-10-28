//! The Diagnostics card's one persisted setting.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// What this buys is the next launch, which `logging::install` starts at the level the caller has
/// already set live.
pub fn set_verbose_logging(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.diagnostics.verbose_logging = on;
    })
}
