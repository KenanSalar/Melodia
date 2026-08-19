//! Linux package-format detection, and the install command that follows from it.
//!
//! A `.rpm` or `.deb` installed directly, with no repository behind it, lands
//! the binary at a root-owned path indistinguishable from a tarball under
//! `/opt/`. Detecting which format owns it is what lets the manifest lookup ask
//! for the packaged asset rather than the tarball, and what routes the verified
//! file through `pkexec dnf install` at swap time.
//!
//! Detection probes `rpm -qf` then `dpkg -S`, both O(1) lookups against a local
//! database, and **memoises** the result — package ownership of a running binary
//! can't change mid-process. Command resolution is deliberately **not** cached:
//! it costs one `PATH` walk per install attempt, and re-resolving covers the user
//! who installs a newer `dnf5` mid-session.
//!
//! An `AppImage` short-circuits on `$APPIMAGE`. Its squashfs mount is owned by
//! nothing by definition, and skipping the spawn keeps "not owned by any
//! package" out of the log.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use super::install_target;

/// The two formats the in-app updater can install through `pkexec`. Anything
/// else — `pacman`, a manual `/opt/` drop — answers `None` from [`detect`] and
/// falls through to the tarball path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPackageFormat {
    Rpm,
    Deb,
}

/// The unset cell means "no probe yet"; the inner `Option` is the answer.
/// Skipped under `cfg(test)`, so the test binary's own path can't poison a
/// later run that wants to override the probe.
static CACHED: OnceLock<Option<LinuxPackageFormat>> = OnceLock::new();

/// The package format owning the running binary, or `None` when no probe
/// matches — where the caller falls back to the tarball asset, or hides the
/// in-app updater outright.
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
    // Exits 0 when the file is owned by an installed package. A missing `rpm`
    // reads as "not owned", which is correct — there is no `dnf` to elevate
    // through without it.
    Command::new("rpm")
        .arg("-qf")
        .arg("--")
        .arg(path)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn owned_by_dpkg(path: &PathBuf) -> bool {
    // `dpkg -S <path>` exits 0 when the file is owned by an installed
    // package. Note: `dpkg -S` matches by path-suffix in its db, so
    // pass the absolute path — `install_target()` always does.
    Command::new("dpkg")
        .arg("-S")
        .arg("--")
        .arg(path)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The program to drive an elevated install of `format`, or `None` when no
/// candidate is on `$PATH` — which the caller surfaces as an error rather than
/// guessing.
///
/// `dnf5` is preferred over `dnf`, Fedora deprecating the legacy shim, and
/// `apt` over `apt-get`, which stays as the scripting-stable fallback. Each pair
/// takes a local package path identically.
pub fn resolve_install_program(format: LinuxPackageFormat) -> Option<&'static str> {
    let candidates: &[&'static str] = match format {
        LinuxPackageFormat::Rpm => &["dnf5", "dnf"],
        LinuxPackageFormat::Deb => &["apt", "apt-get"],
    };
    candidates.iter().copied().find(|cmd| program_exists(cmd))
}

fn program_exists(name: &str) -> bool {
    // Walked rather than spawned: executing a binary to learn it exists is
    // wasteful and its `--version` flag varies, where a few `stat`s keep the
    // install path side-effect free until the user actually clicks Install.
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

/// `true` when `path` is a regular file with at least one execute bit set. On
/// non-Unix — compiled but unreachable — a plain file check.
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
