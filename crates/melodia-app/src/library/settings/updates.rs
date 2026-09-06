//! Updater-state setters. Every setter funnels through
//! [`crate::services::settings::mutate_settings`] so a burst of writes
//! during a check loop (success/failure followed by a skip-clear after
//! the manifest is parsed) can't tear each other's reads.

use chrono::{DateTime, Utc};

use crate::services;
use crate::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

/// Persist the user toggle for "Automatically check for updates". The
/// runtime effect (whether `tasks::updater_daily::spawn` registers a
/// background loop at all) is consulted once at startup; toggling here
/// takes effect on the next launch.
pub fn set_auto_check_enabled(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.updates.auto_check_enabled = on;
    })
}

/// Persist a successful manifest fetch. What it does to the flags is
/// [`UpdateFlags::record_success`](crate::services::settings::UpdateFlags::record_success),
/// which is where the `304` case is argued.
pub fn record_check_success(
    state: &AppState,
    now: DateTime<Utc>,
    latest_version: Option<String>,
    etag: Option<String>,
) -> Result<(), AppError> {
    write_check_success(&state.paths, now, latest_version, etag)
}

/// [`record_check_success`]'s body, narrowed so what the flags do to the file can be driven.
fn write_check_success(
    paths: &Paths,
    now: DateTime<Utc>,
    latest_version: Option<String>,
    etag: Option<String>,
) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.updates.record_success(now.timestamp(), latest_version, etag);
    })
}

/// Persist a failed manifest fetch. The daily task swaps to a longer re-arm cadence once the
/// counter reaches 3, reverting to 6h on the next successful check.
pub fn record_check_failure(state: &AppState, now: DateTime<Utc>) -> Result<(), AppError> {
    write_check_failure(&state.paths, now)
}

/// [`record_check_failure`]'s body, narrowed beside its sibling.
fn write_check_failure(paths: &Paths, now: DateTime<Utc>) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.updates.record_failure(now.timestamp());
    })
}

/// User clicked "Skip this version" in Settings → Updates. The version
/// string lands in `skipped_release`; the daily-check loop's notify
/// gate skips a toast as long as the live manifest reports this exact
/// version. A strictly-newer version clears the skip via
/// [`reset_skipped_release`] (callsite: the loop's notify gate).
pub fn set_skipped_release(state: &AppState, version: String) -> Result<(), AppError> {
    write_skipped_release(&state.paths, version)
}

/// [`set_skipped_release`]'s body, narrowed so the pair with [`clear_skipped_release`] can be
/// driven end to end.
fn write_skipped_release(paths: &Paths, version: String) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.updates.skipped_release = version;
    })
}

/// Clear `skipped_release`. Called from the daily-check loop when the
/// fresh manifest reports a version newer than the user's previous
/// skip — at that point the skip becomes stale and the user should see
/// the new version's toast.
pub fn reset_skipped_release(state: &AppState) -> Result<(), AppError> {
    clear_skipped_release(&state.paths)
}

/// [`reset_skipped_release`]'s body, narrowed beside its sibling.
fn clear_skipped_release(paths: &Paths) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, |settings| {
        settings.updates.skipped_release.clear();
    })
}

#[cfg(test)]
#[path = "tests/updates_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/updates_writers_tests.rs"]
mod writer_tests;
