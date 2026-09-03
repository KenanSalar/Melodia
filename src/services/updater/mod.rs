//! Auto-updater backend.
//!
//! The pipeline: the daily task's `check_for_update` fetches `latest.json`
//! ETag-aware, `is_upgrade` decides on semver, and an accepted notification runs
//! `download_and_install` → `verify_stream` (prehashed minisign) → `swap_in_place`
//! (atomic, `cfg`-branched) → `request_respawn_and_quit`.
//!
//! The manifest schema lives in [`manifest`] and signature verification in
//! [`minisign`]. The threat model's trust boundary is the GitHub repo: `latest.json`
//! and every artifact are minisign-signed with the key embedded at
//! `assets/updater-pubkey.b64`, and the client fails closed on a missing or invalid
//! signature.

use std::path::PathBuf;

use crate::error::AppResult;

pub mod asset_cache;
pub mod check;
pub mod event;
pub mod github;
pub mod install;
pub mod manifest;
pub mod minisign;
pub mod version;

pub use check::{CheckOutcome, check_for_update};
pub use event::{FailureKind, UpdaterEvent};
pub use install::{download_and_install, prune_stale_staging};
// Re-exported from `platform::install_kind`, whose two other consumers are why it left this
// directory. Kept here so the UI keeps asking the updater whether an install is a system one.
pub use crate::services::platform::install_kind::{install_target, is_system_install};

/// Whether this build has an in-app updater at all.
///
/// A source build doesn't: `target/` belongs to cargo, so a swapped-in release is older than the
/// tree above it and gone at the next build. Where [`is_system_install`] keeps the check and only
/// trades the install button for a package-manager hint, this takes the whole section.
#[must_use]
pub fn is_available() -> bool {
    !crate::utils::exe::is_dev_build()
}

/// `<install_target>.old` — the rollback copy [`install::swap_in_place`] retains on
/// Linux atomic-swap installs (`AppImage` / tarball), so a failed post-swap smoke
/// test can restore it before the user sees a broken installation and a successful
/// boot can reap it from `main()`. Errors only if [`install_target`] does.
///
/// Linux-only: a Windows install is `msiexec /i` of a signed MSI, so Windows
/// Installer's `MajorUpgrade` + Restart Manager own the replace and no `.old` is
/// ever produced. Derived through [`install::old_path`], which keeps the swap path
/// and the reaping path bit-identical.
#[cfg(target_os = "linux")]
pub fn install_target_old() -> AppResult<PathBuf> {
    let target = install_target()?;
    Ok(install::old_path(&target))
}

#[cfg(test)]
#[path = "tests/availability_tests.rs"]
mod tests;
