//! Linux package-format detection, and the install command that follows from it.
//!
//! A `.rpm` or `.deb` installed directly lands at a root-owned path indistinguishable from a
//! tarball under `/opt/`, so detecting the owner is what asks the manifest for the packaged asset
//! and routes the swap through `pkexec dnf install`.
//!
//! Detection is memoised, a running binary's owner being unable to change mid-process. Command
//! resolution isn't: one `PATH` walk per install covers a `dnf5` installed mid-session.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use super::install_target;

/// What the updater can install through `pkexec`. Anything else, `pacman` or a manual `/opt/`
/// drop, answers `None` and takes the tarball path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPackageFormat {
    Rpm,
    Deb,
}

/// Skipped under `cfg(test)`, so the test binary's own path can't poison a later override. That
/// reaches only **this crate's** tests, `cfg(test)` not crossing crates.
static CACHED: OnceLock<Option<LinuxPackageFormat>> = OnceLock::new();

/// The package format owning the running binary, or `None` where no probe matches.
pub fn detect() -> Option<LinuxPackageFormat> {
    if cfg!(test) {
        return probe();
    }
    *CACHED.get_or_init(probe)
}

fn probe() -> Option<LinuxPackageFormat> {
    if !cfg!(target_os = "linux") {
        return None;
    }

    // A squashfs mount nothing owns; skipping the spawn keeps a "not owned" line out of the log.
    if std::env::var("APPIMAGE").is_ok_and(|p| !p.is_empty()) {
        return None;
    }

    let target = install_target().ok()?;

    if owned_by_rpm(&target) {
        return Some(LinuxPackageFormat::Rpm);
    }
    if owned_by_dpkg(&target) {
        return Some(LinuxPackageFormat::Deb);
    }
    None
}

fn owned_by_rpm(path: &PathBuf) -> bool {
    // A missing `rpm` reads as not owned, which is right: there is no `dnf` to elevate through.
    Command::new("rpm").arg("-qf").arg("--").arg(path).output().is_ok_and(|o| o.status.success())
}

fn owned_by_dpkg(path: &PathBuf) -> bool {
    // `dpkg -S` matches by path suffix, hence the absolute path `install_target()` gives.
    Command::new("dpkg").arg("-S").arg("--").arg(path).output().is_ok_and(|o| o.status.success())
}

/// The program for an elevated install of `format`, or `None` where none is on `$PATH`, which the
/// caller reports rather than guessing. `dnf5` over the deprecated `dnf` shim, `apt` over the
/// scripting-stable `apt-get`.
pub fn resolve_install_program(format: LinuxPackageFormat) -> Option<&'static str> {
    let candidates: &[&'static str] = match format {
        LinuxPackageFormat::Rpm => &["dnf5", "dnf"],
        LinuxPackageFormat::Deb => &["apt", "apt-get"],
    };
    candidates.iter().copied().find(|cmd| program_exists(cmd))
}

fn program_exists(name: &str) -> bool {
    // Walked rather than spawned, keeping the path side-effect free until the user clicks Install.
    let Ok(path_var) = std::env::var("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return true;
        }
    }
    false
}

/// A regular file with an execute bit set; off Unix, compiled but unreachable, a plain file check.
fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
#[path = "tests/linux_pkg_tests.rs"]
mod tests;
