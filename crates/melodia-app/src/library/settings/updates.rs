//! The two updater settings the user sets. What a check records is
//! [`run_check`](crate::services::updater::run_check)'s, written as one change.

use crate::services;
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Persist the user toggle for "Automatically check for updates". The daily
/// loop reads it per tick; the caller bumps `AppState::auto_check_changed`
/// on `Ok` so a sleeping loop doesn't wait out its cadence to see it.
pub fn set_auto_check_enabled(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.updates.auto_check_enabled = on;
    })
}

/// User clicked "Skip this version" in Settings → Updates. Either check stays
/// quiet about this exact version unless it is critical, and clears the skip
/// once a strictly newer one is published.
pub fn set_skipped_release(state: &AppState, version: String) -> Result<(), AppError> {
    write_skipped_release(&state.paths, version)
}

/// [`set_skipped_release`]'s body, narrowed so it can be driven through the file.
fn write_skipped_release(paths: &Paths, version: String) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.updates.skipped_release = version;
    })
}

#[cfg(test)]
#[path = "tests/updates_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/updates_writers_tests.rs"]
mod writer_tests;
