//! The rating write-back switch. Persists to `settings.json`; the in-memory shadow on
//! [`AppState`] is refreshed by the UI callback before the write, the same synchronous-shadow
//! ordering [`super::radio`] uses.

use crate::error::AppError;
use crate::services;
use crate::state::AppState;

/// Persist whether a star set in Melodia is also written into the file's own tag.
pub fn set_write_ratings_to_tags(state: &AppState, write: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.library.write_ratings_to_tags = write;
    })
}
