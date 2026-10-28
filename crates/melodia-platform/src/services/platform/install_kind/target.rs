//! The `latest.json` `platforms{}` key for this binary: one per install format, so every Linux
//! format updates in place without the server disambiguating.

#[cfg(target_os = "linux")]
use super::linux_pkg;
use super::linux_pkg::LinuxPackageFormat;

/// `None` on a host that isn't packaged, which the caller treats as no asset for the target.
pub fn current_target_key() -> Option<&'static str> {
    format_key(host_os()?, host_arch()?, appimage_env_set(), current_pkg_format())
}

/// Pure over its inputs, so every `(os, arch, appimage, pkg)` combination is table-tested on the
/// host arch rather than only the one CI runs on. `os` and `arch` are `std::env::consts` tokens,
/// and off Linux `appimage_env` is `false` and `pkg` is `None`.
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

/// Cheapest probe first: `$APPIMAGE`, then package ownership, then the tarball.
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

/// `None` off Linux and Windows, so the wrapper never runs the Linux-only package probe there.
fn host_os() -> Option<&'static str> {
    if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

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
