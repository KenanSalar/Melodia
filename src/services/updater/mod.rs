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
pub mod linux_pkg;
pub mod manifest;
pub mod minisign;
mod probe;
pub mod state;
pub mod system_install;
pub mod target;
pub mod version;

pub use check::{CheckOutcome, check_for_update};
pub use event::{FailureKind, UpdaterEvent};
pub use install::{download_and_install, prune_stale_staging};
pub use state::UpdaterState;
pub use system_install::is_system_install;

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

/// The file the swap actually replaces. On an `AppImage` run the executable path is
/// the read-only squashfs mount and the replaceable file is at `$APPIMAGE`; every
/// path-touching module here routes through this, the **only** function in the
/// updater that asks for the running binary's path.
///
/// The other arm goes through [`crate::services::current_exe`] rather than
/// `std::env::current_exe()`, which on Linux hands back a `<path> (deleted)` string
/// once the binary has been replaced on disk — an RPM/DEB upgrade mid-session is
/// exactly that, and this answer is what `desktop_integration` bakes into the user's
/// `Exec=` line and what `linux_pkg::detect` looks up in the package database.
pub fn install_target() -> AppResult<PathBuf> {
    if cfg!(target_os = "linux")
        && let Ok(p) = std::env::var("APPIMAGE")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    Ok(crate::services::current_exe()?)
}
