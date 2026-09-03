//! Which install this is, and where the file a swap would replace actually lives.
//!
//! Split out of `updater/` because it has a second consumer, which is the only evidence that
//! separates a platform primitive from a feature's internals: `crash_report` stamps
//! [`target::current_target_key`] into a report and `desktop_integration` bakes
//! [`install_target`] into a `.desktop` `Exec=` line. Everything the updater does *with* these
//! answers stays in the updater.

use std::path::PathBuf;

use melodia_core::error::AppResult;

pub mod linux_pkg;
pub mod probe;
pub mod system_install;
pub mod target;

pub use system_install::is_system_install;

/// The file the swap actually replaces. On an `AppImage` run the executable path is
/// the read-only squashfs mount and the replaceable file is at `$APPIMAGE`; every
/// path-touching module in the updater routes through this, the **only** function
/// there or here that asks for the running binary's path.
///
/// The other arm goes through [`melodia_core::utils::exe::current_exe`] rather than
/// `std::env::current_exe()`, which on Linux hands back a `<path> (deleted)` string
/// once the binary has been replaced on disk — an RPM/DEB upgrade mid-session is
/// exactly that, and this answer is what `desktop_integration` bakes into the user's
/// `Exec=` line and what [`linux_pkg::detect`] looks up in the package database.
pub fn install_target() -> AppResult<PathBuf> {
    if cfg!(target_os = "linux")
        && let Ok(p) = std::env::var("APPIMAGE")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    Ok(melodia_core::utils::exe::current_exe()?)
}
