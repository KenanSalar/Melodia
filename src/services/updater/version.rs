//! Semver-based "is this candidate version strictly newer than current?"
//! check. String equality on `latest.json`'s `version` field would let a
//! compromised or accidentally rolled-back manifest force a downgrade —
//! the daily-check loop calls this before transitioning to `Available`.

use semver::Version;

use crate::error::{AppError, AppResult};

/// Returns `Ok(true)` only when `candidate` parses as a strictly higher
/// semver than `current`. Equal and older candidates return `Ok(false)`
/// (no notification, no install). Malformed inputs surface as
/// `AppError::Validation` rather than silently allowing the upgrade.
pub fn is_upgrade(current: &str, candidate: &str) -> AppResult<bool> {
    let current = Version::parse(current).map_err(|e| {
        AppError::Validation(format!("current version '{current}' is not valid semver: {e}"))
    })?;
    let candidate = Version::parse(candidate).map_err(|e| {
        AppError::Validation(format!("candidate version '{candidate}' is not valid semver: {e}"))
    })?;
    Ok(candidate > current)
}

#[cfg(test)]
#[path = "tests/version_tests.rs"]
mod tests;
