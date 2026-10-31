//! Which install this is, and where the file a swap would replace lives. Here rather than in
//! `updater/` because `crash_report` and `desktop_integration` ask too; what the updater does with
//! the answers stays in the updater.

use std::path::PathBuf;

use melodia_core::error::AppResult;

pub mod linux_pkg;
mod probe;
pub mod system_install;
pub mod target;

pub use system_install::is_system_install;

/// The file a swap replaces, and the **only** place the updater or this module asks for the
/// running binary's path. Under an `AppImage` that is `$APPIMAGE`, the executable path being the
/// read-only mount.
///
/// Otherwise [`melodia_core::utils::exe::current_exe`], never `std::env::current_exe()`, whose
/// answer gains `" (deleted)"` once an RPM/DEB upgrade replaces the binary mid-session, and this is
/// what `desktop_integration` writes into `Exec=` and [`linux_pkg::detect`] looks up. A marked
/// path makes `rpm -qf` miss, which compounds into a tarball offered to an RPM install and a wrong
/// `install` line in crash reports.
pub fn install_target() -> AppResult<PathBuf> {
    if cfg!(target_os = "linux")
        && let Ok(p) = std::env::var("APPIMAGE")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    Ok(melodia_core::utils::exe::current_exe()?)
}
