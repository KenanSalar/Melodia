//! Per-format target key selection — picks the `platforms{}` map entry
//! that matches the current binary's platform + install format.
//!
//! Per-format keys (`linux-x86_64-appimage` vs `linux-x86_64-tarball`
//! vs `linux-x86_64-rpm` vs `linux-x86_64-deb`) rather than Tauri-style
//! flat `linux-x86_64` lets every Linux install format update in-place
//! without server-side disambiguation. Format selection priority is
//! cheapest-probe-first: `$APPIMAGE` env var (free), then
//! [`super::linux_pkg::detect`] (one subprocess), then tarball
//! fallback.
//!
//! Architectures: `x86_64` + `aarch64`. The arch token in the key
//! (`-x86_64-` / `-aarch64-`) is set at compile time via `cfg!`; per-arch
//! branches reuse the same format-selection logic so adding a new
//! arch is a single helper call.
//!
//! The mapping logic lives in [`format_key`] — a pure function over
//! `(os, arch, appimage_env, pkg_format)`. `current_target_key()` is a
//! thin wrapper that supplies those four values from `cfg!` + runtime
//! probes. Keeping the mapping pure means every `(os, arch, appimage,
//! pkg)` combination is exercised by host-arch tests, not just the
//! arch the CI runner happens to be.

#[cfg(target_os = "linux")]
use super::linux_pkg;
use super::linux_pkg::LinuxPackageFormat;

/// Returns the `latest.json` `platforms{}` key for the running binary,
/// or `None` if the host platform isn't packaged (currently: anything
/// other than `x86_64` / `aarch64` on Linux or Windows).
pub fn current_target_key() -> Option<&'static str> {
    format_key(host_os()?, host_arch()?, appimage_env_set(), current_pkg_format())
}

/// Pure mapping from environment inputs to `latest.json` `platforms{}`
/// key. Extracted so every `(os, arch, appimage, pkg)` combination can
/// be table-tested on the host arch without `#[cfg(target_arch = ...)]`
/// gating each branch (the previous shape silently left aarch64 keys
/// untested on `x86_64` CI runners).
///
/// `os` and `arch` are the lowercase tokens from `std::env::consts`
/// (`"linux"`, `"windows"`, `"x86_64"`, `"aarch64"`). `appimage_env` is
/// `true` when `$APPIMAGE` is set to a non-empty value (Linux only;
/// ignored on Windows, where the arg is always passed `false`). `pkg`
/// is the result of [`super::linux_pkg::detect`] on Linux and `None`
/// elsewhere — non-Linux callers must pass `None`.
///
/// Returns `None` for unsupported platforms (macOS, BSD, riscv, …).
/// The caller treats that the same as "no asset for target": the UI
/// hides the updater and the daily task short-circuits.
pub(crate) fn format_key(
    os: &str,
    arch: &str,
    appimage_env: bool,
    pkg: Option<LinuxPackageFormat>,
) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some(linux_format_key("x86_64", appimage_env, pkg)),
        ("linux", "aarch64") => Some(linux_format_key("aarch64", appimage_env, pkg)),
        ("windows", "x86_64") => Some("windows-x86_64-msi"),
        ("windows", "aarch64") => Some("windows-aarch64-msi"),
        _ => None,
    }
}

/// Linux format-selection: `$APPIMAGE` wins (cheapest probe), then
/// package-manager ownership, then tarball fallback. The arch token is
/// burned into each return value so the caller never has to format-
/// substitute.
///
/// Note on Windows: the in-app updater downloads the signed `.msi` and
/// hands it to `msiexec /i` — UAC prompts, the `MajorUpgrade` element
/// in `wix/main.wxs` replaces this version, and Windows Installer's
/// Restart Manager (registered for `Melodia.exe` via
/// `util:RestartResource`) closes the running app before the swap.
/// Architecturally this mirrors the Linux RPM/DEB branch: no `.old`
/// snapshot, no in-process rollback, no smoke test — the package
/// format owns the install.
fn linux_format_key(
    arch: &'static str,
    appimage_env: bool,
    pkg: Option<LinuxPackageFormat>,
) -> &'static str {
    if appimage_env {
        return match arch {
            "aarch64" => "linux-aarch64-appimage",
            _ => "linux-x86_64-appimage",
        };
    }
    match (pkg, arch) {
        (Some(LinuxPackageFormat::Rpm), "aarch64") => "linux-aarch64-rpm",
        (Some(LinuxPackageFormat::Rpm), _) => "linux-x86_64-rpm",
        (Some(LinuxPackageFormat::Deb), "aarch64") => "linux-aarch64-deb",
        (Some(LinuxPackageFormat::Deb), _) => "linux-x86_64-deb",
        (None, "aarch64") => "linux-aarch64-tarball",
        (None, _) => "linux-x86_64-tarball",
    }
}

/// Compile-time host OS token, narrowed to the set [`format_key`]
/// understands. Returns `None` on macOS / BSD / etc. so the wrapper
/// short-circuits without invoking the (Linux-only) package probe.
fn host_os() -> Option<&'static str> {
    if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

/// Compile-time host arch token, narrowed to the supported set.
fn host_arch() -> Option<&'static str> {
    if cfg!(target_arch = "x86_64") {
        Some("x86_64")
    } else if cfg!(target_arch = "aarch64") {
        Some("aarch64")
    } else {
        None
    }
}

fn appimage_env_set() -> bool {
    std::env::var("APPIMAGE").is_ok_and(|v| !v.is_empty())
}

#[cfg(target_os = "linux")]
fn current_pkg_format() -> Option<LinuxPackageFormat> {
    linux_pkg::detect()
}

#[cfg(not(target_os = "linux"))]
fn current_pkg_format() -> Option<LinuxPackageFormat> {
    None
}

#[cfg(test)]
#[path = "tests/target_tests.rs"]
mod tests;
