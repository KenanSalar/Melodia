//! Auto-updater backend.
//!
//! Wire shape:
//! ```text
//!   daily task ── check_for_update ──► fetch latest.json (ETag-aware)
//!                                         │
//!                                         ▼
//!                                    is_upgrade (semver)
//!                                         │
//!                                         ▼
//!                                       notify
//!                                         │
//!                                         ▼
//!                                  download_and_install
//!                                         │
//!                                         ▼
//!                                    verify_stream (prehashed minisign)
//!                                         │
//!                                         ▼
//!                                    swap_in_place (atomic; cfg-branched)
//!                                         │
//!                                         ▼
//!                                    request_respawn + quit_event_loop
//! ```
//!
//! The manifest schema lives in [`manifest`], signature verification in
//! [`minisign`], and the threat model's trust boundary is the GitHub repo:
//! `latest.json` and every artifact are minisign-signed with the key embedded
//! at `assets/updater-pubkey.b64`, and the client fails closed on a missing
//! or invalid signature.

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

/// `<install_target>.old` — the rollback copy of the previously-running
/// binary, retained by [`install::swap_in_place`] on Linux atomic-swap
/// installs (`AppImage` / tarball) so (a) a failed post-swap smoke test
/// can restore it before the user sees a broken installation and (b) a
/// successful boot of the new binary can reap it from `main()`. Returns
/// an error only if [`install_target`] itself fails (e.g. `current_exe()`
/// denies, which is essentially never).
///
/// Linux-only: Windows installs flow through `msiexec /i` of a signed
/// MSI (`InstallMethod::WindowsMsi`), which lets Windows Installer's
/// `MajorUpgrade` + Restart Manager own the replace — no `.old`
/// snapshot is ever produced at the install target.
///
/// Path derivation is shared with [`install::old_path`] to keep the
/// swap path and the reaping path bit-identical.
#[cfg(target_os = "linux")]
pub fn install_target_old() -> AppResult<PathBuf> {
    let target = install_target()?;
    Ok(install::old_path(&target))
}

/// Resolves to the file the swap actually replaces. On `AppImage` runs
/// `std::env::current_exe()` returns the read-only squashfs mount path
/// (`/tmp/.mount_*/usr/bin/Melodia`); the replaceable `AppImage` file
/// itself is at `$APPIMAGE`. Every path-touching module in this tree
/// routes through this — it's the **only** function in the updater
/// that calls `std::env::current_exe()` directly.
pub fn install_target() -> AppResult<PathBuf> {
    if cfg!(target_os = "linux")
        && let Ok(p) = std::env::var("APPIMAGE")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    Ok(std::env::current_exe()?)
}
