//! Linux package-format detection + install-command resolution.
//!
//! When a user installs Melodia from a `.rpm` or `.deb` they downloaded
//! directly (no `dnf`/`apt` repository), the binary lands at
//! `/usr/bin/melodia` — root-owned, the same path a tarball under
//! `/opt/` would take. To let the in-app updater drive an upgrade for
//! these users we need to:
//!
//!   1. **Detect** which package format owns the running binary so the
//!      manifest lookup in [`super::target::current_target_key`] can
//!      ask for `linux-x86_64-rpm` or `linux-x86_64-deb` instead of the
//!      tarball.
//!   2. **Resolve** the right elevated install command at swap time so
//!      [`super::install::download_and_install`] can route the verified
//!      `.rpm`/`.deb` through `pkexec dnf install` /
//!      `pkexec apt install`.
//!
//! Detection probes `rpm -qf <install_target>` first, then
//! `dpkg -S <install_target>`. Both are O(1) lookups against a local
//! database — typical wall-time well under 50 ms. Probe result is
//! memoised in a `OnceLock` since the package ownership of the running
//! binary can't change during a process lifetime (modulo a `dnf
//! reinstall` mid-run, which would leave the open fd intact anyway).
//!
//! Command resolution is **not** cached — the cost is one
//! `std::env::split_paths` walk per install attempt, which only happens
//! when the user clicks "Download & install". Re-resolving keeps the
//! code shorter and dodges the corner case where the user installs a
//! newer `dnf5` mid-session.
//!
//! `AppImage` runs are short-circuited via `$APPIMAGE`: the squashfs
//! mount path is by definition not owned by `rpm`/`dpkg`, but skipping
//! the subprocess spawn keeps boot quiet and avoids polluting logs with
//! "not owned by any package" stderr.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use super::install_target;

/// The two Linux package formats the in-app updater knows how to
/// install via `pkexec`. Anything else (Arch `pacman`, Slackware,
/// genuinely manual `/opt/` drops) returns `None` from [`detect`] and
/// falls through to the existing tarball path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxPackageFormat {
    Rpm,
    Deb,
}

/// Cached detection result. `None` means "no probe yet"; the inner
/// `Option<LinuxPackageFormat>` is the actual answer (`None` = not
/// package-owned). Skipped under `cfg(test)` so the test binary at
/// `target/debug/deps/<name>` doesn't poison subsequent runs that may
/// want to override probe behaviour.
static CACHED: OnceLock<Option<LinuxPackageFormat>> = OnceLock::new();

/// Returns the package format that owns the running binary, or `None`
/// when no probe matches.
///
/// - **Linux + `AppImage`** → `None` (squashfs mount, never package-owned).
/// - **Linux + RPM-owned binary** → `Some(Rpm)`.
/// - **Linux + dpkg-owned binary** → `Some(Deb)`.
/// - **Linux + unknown** → `None`. Caller falls back to the tarball
///   asset or hides the in-app updater entirely (see
///   `system_install.rs`).
/// - **Non-Linux** → `None` (compiled out).
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
    // `rpm -qf <path>` exits 0 when the file is owned by an installed
    // package and prints the NEVRA on stdout. Non-zero exit (1 is the
    // canonical "not owned" code) means no match. A missing `rpm`
    // binary returns `Err` from `output()` — treated as "not owned",
    // which is correct because we can't elevate via `dnf` without it.
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

/// Returns the `(program, must-precede-args)` pair to drive an elevated
/// install of `format` via `pkexec`. The caller appends the staged
/// package path.
///
/// Preference order:
///   - **RPM**: `dnf5` → `dnf`. Both expose `install -y <local.rpm>`
///     identically; `dnf5` (Fedora 41+) is preferred because Fedora is
///     moving the legacy `dnf` shim to a deprecated state.
///   - **DEB**: `apt` → `apt-get`. Modern `apt` is the user-facing
///     wrapper introduced in apt 1.0 (2014); `apt-get` remains as the
///     scripting-stable fallback. Both accept absolute paths to local
///     `.deb` files as of apt 1.1 (2015), well below any supported
///     distro's apt version.
///
/// Returns `None` when no candidate is on `$PATH`. The caller surfaces
/// that as a "no supported package manager" error rather than guessing.
pub fn resolve_install_program(format: LinuxPackageFormat) -> Option<&'static str> {
    let candidates: &[&'static str] = match format {
        LinuxPackageFormat::Rpm => &["dnf5", "dnf"],
        LinuxPackageFormat::Deb => &["apt", "apt-get"],
    };
    candidates.iter().copied().find(|cmd| program_exists(cmd))
}

fn program_exists(name: &str) -> bool {
    // PATH lookup without spawning. `Command::new(name).status()` would
    // execute the binary just to find out if it exists — wasteful, and
    // `--version` calls vary by program. Walking `$PATH` ourselves is a
    // few stat() calls and keeps the install code path side-effect free
    // until the user actually clicks "Install".
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

/// `true` when `path` resolves to a regular file the current process
/// could `exec(3)` — i.e. the file exists and at least one execute bit
/// is set. On non-Unix builds (compiled but unreachable at runtime —
/// `resolve_install_program` only meaningfully runs on Linux) this
/// reduces to a plain file check.
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
