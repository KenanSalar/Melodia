//! Updater-state setters. Every setter funnels through
//! [`crate::services::settings::mutate_settings`] so a burst of writes
//! during a check loop (success/failure followed by a skip-clear after
//! the manifest is parsed) can't tear each other's reads.

use chrono::{DateTime, Utc};

use crate::error::AppError;
use crate::services;
use crate::state::AppState;

/// Persist the user toggle for "Automatically check for updates". The
/// runtime effect (whether `tasks::updater_daily::spawn` registers a
/// background loop at all) is consulted once at startup; toggling here
/// takes effect on the next launch.
pub fn set_auto_check_enabled(state: &AppState, on: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.updates.auto_check_enabled = on;
    })
}

/// Record a successful manifest fetch. Resets `consecutive_failures` to
/// zero (mitigates flaky-network thrash that bumped it earlier in the
/// session), updates the `ETag` so the next loop iteration can short-circuit
/// on `304`, and stores the latest version we observed.
///
/// `latest_version` is `None` when the fetch returned `304 Not Modified` —
/// the cached version is still the most-recently-seen value, so we leave
/// `last_known_release` untouched.
pub fn record_check_success(
    state: &AppState,
    now: DateTime<Utc>,
    latest_version: Option<String>,
    etag: Option<String>,
) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.updates.last_check_unix = now.timestamp();
        settings.updates.consecutive_failures = 0;
        if let Some(v) = latest_version {
            settings.updates.last_known_release = v;
        }
        if let Some(tag) = etag {
            settings.updates.last_manifest_etag = tag;
        }
    })
}

/// Record a failed manifest fetch. Increments `consecutive_failures`
/// (saturating at `u8::MAX`) and updates `last_check_unix` so the loop's
/// 24h elapsed gate still advances — otherwise repeated failures would
/// re-fire on every loop iteration. The daily task swaps to a 7d re-arm
/// cadence once the counter reaches 3, reverting to 6h on the next
/// successful check.
pub fn record_check_failure(state: &AppState, now: DateTime<Utc>) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.updates.last_check_unix = now.timestamp();
        settings.updates.consecutive_failures =
            settings.updates.consecutive_failures.saturating_add(1);
    })
}

/// User clicked "Skip this version" in Settings → Updates. The version
/// string lands in `skipped_release`; the daily-check loop's notify
/// gate skips a toast as long as the live manifest reports this exact
/// version. A strictly-newer version clears the skip via
/// [`reset_skipped_release`] (callsite: the loop's notify gate).
pub fn set_skipped_release(state: &AppState, version: String) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.updates.skipped_release = version;
    })
}

/// Clear `skipped_release`. Called from the daily-check loop when the
/// fresh manifest reports a version newer than the user's previous
/// skip — at that point the skip becomes stale and the user should see
/// the new version's toast.
pub fn reset_skipped_release(state: &AppState) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, |settings| {
        settings.updates.skipped_release.clear();
    })
}

#[cfg(test)]
#[path = "tests/updates_tests.rs"]
mod tests;
