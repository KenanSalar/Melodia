//! The two updater settings the user sets. What a check records is
//! [`run_check`](crate::services::updater::run_check)'s, written as one change.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Persist the user toggle for "Automatically check for updates". The daily
/// loop reads it per tick; the caller bumps `AppState::auto_check_changed`
/// on `Ok` so a sleeping loop doesn't wait out its cadence to see it.
pub fn set_auto_check_enabled(paths: &Paths, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.updates.auto_check_enabled = on;
    })
}

/// User clicked "Skip this version" in Settings → Updates. Either check stays
/// quiet about this exact version unless it is critical, and clears the skip
/// once a strictly newer one is published.
pub fn set_skipped_release(paths: &Paths, version: String) -> Result<(), AppError> {
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
