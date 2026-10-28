use semver::Version;

use melodia_core::error::{AppError, AppResult};

/// Whether `candidate` is strictly newer than `current` by semver, so a rolled-back manifest can't
/// force a downgrade. Malformed input is an error rather than an upgrade.
pub fn is_upgrade(current: &str, candidate: &str) -> AppResult<bool> {
    let current = Version::parse(current).map_err(|e| {
        AppError::updater(format!("current version '{current}' is not valid semver"), e)
    })?;
    let candidate = Version::parse(candidate).map_err(|e| {
        AppError::updater(format!("candidate version '{candidate}' is not valid semver"), e)
    })?;
    Ok(candidate > current)
}

#[cfg(test)]
#[path = "tests/version_tests.rs"]
mod tests;
