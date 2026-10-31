//! The in-app updater. Its trust boundary is the GitHub repo: `latest.json` and every artifact
//! are minisign-signed with the key in `assets/updater-pubkey.b64`, and a missing or invalid
//! signature fails closed.

#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use melodia_core::error::AppResult;

pub mod asset_cache;
pub mod check;
pub mod event;
pub mod github;
pub mod install;
pub mod manifest;
pub mod minisign;
pub mod run;
pub mod version;

#[cfg(target_os = "linux")]
use melodia_platform::services::platform::install_kind::install_target;

pub use check::{Checked, Verdict, check_for_update};
pub use event::{FailureKind, UpdateFailure, UpdateResult, UpdaterEvent};
pub use github::RELEASES_BASE;
pub use install::{
    InstallOutcome, download_and_install, installed_version, installer_running, prune_stale_staging,
};
pub use run::{Finding, PanelPaint, run_check};

/// Whether this build has an in-app updater at all: not a source build, whose `target/` belongs to
/// cargo and would lose a swapped-in release at the next build. A gate of its own rather than a
/// case of `is_system_install`, whose package-manager hint `cargo run` would then be offered.
#[must_use]
pub fn is_available() -> bool {
    !melodia_core::utils::exe::is_dev_build()
}

/// `<install_target>.old`, the rollback copy a Linux swap keeps until a successful boot reaps it
/// from `main()`. Linux-only: Windows installs through msiexec and keeps none.
#[cfg(target_os = "linux")]
pub fn install_target_old() -> AppResult<PathBuf> {
    let target = install_target()?;
    Ok(install::old_path(&target))
}

#[cfg(test)]
#[path = "tests/availability_tests.rs"]
mod tests;
