//! The rating write-back switch. Persists to `settings.json`; the in-memory shadow on
//! `AppState` is refreshed by the UI callback before the write, the same synchronous-shadow
//! ordering [`super::radio`] uses.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Persist whether a star set in Melodia is also written into the file's own tag.
pub fn set_write_ratings_to_tags(paths: &Paths, write: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.library.write_ratings_to_tags = write;
    })
}
