#![allow(
    unsafe_code,
    reason = "env::set_var/remove_var are unsafe in Rust 2024; ENV_LOCK serialises all sibling tests in this module so concurrent mutations can't race, and prev-value restoration is panic-safe via catch_unwind."
)]

use std::path::PathBuf;

use crate::services::updater::linux_pkg::LinuxPackageFormat;
use crate::services::updater::test_support::APPIMAGE_ENV_LOCK;
use crate::services::updater::{install_target, target::current_target_key, target::format_key};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Helper that lets a test temporarily override `APPIMAGE` for the
/// duration of a closure, restoring whatever was set before. Holds
/// the shared [`APPIMAGE_ENV_LOCK`] for the full duration so sibling
/// tests across the updater test files can't race against each
/// other's `set_var`/`remove_var` calls.
fn with_appimage_env<F: FnOnce()>(value: Option<&str>, body: F) {
    let _guard = APPIMAGE_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prev = std::env::var("APPIMAGE").ok();
    unsafe {
        match value {
            Some(v) => std::env::set_var("APPIMAGE", v),
            None => std::env::remove_var("APPIMAGE"),
        }
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    unsafe {
        match prev {
            Some(p) => std::env::set_var("APPIMAGE", p),
            None => std::env::remove_var("APPIMAGE"),
        }
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

// ---------------------------------------------------------------------------
// Pure `format_key` table tests — these run on every host arch and exercise
// **every** (os, arch, appimage, pkg) combination. They're the safety net
// against typos in the aarch64 key literals: the cfg-gated `current_target_key`
// tests below only run when CI's host arch matches the cfg, which never
// happens for aarch64 on the x86_64 ubuntu-latest runner that drives most
// PR checks.
// ---------------------------------------------------------------------------

#[test]
fn format_key_linux_x86_64_appimage() {
    assert_eq!(
        format_key("linux", "x86_64", true, None),
        Some("linux-x86_64-appimage"),
    );
    // `$APPIMAGE` wins over pkg ownership — the squashfs mount can't be
    // owned by dpkg/rpm, but a buggy probe shouldn't matter.
    assert_eq!(
        format_key("linux", "x86_64", true, Some(LinuxPackageFormat::Rpm)),
        Some("linux-x86_64-appimage"),
    );
    assert_eq!(
        format_key("linux", "x86_64", true, Some(LinuxPackageFormat::Deb)),
        Some("linux-x86_64-appimage"),
    );
}

#[test]
fn format_key_linux_aarch64_appimage() {
    assert_eq!(
        format_key("linux", "aarch64", true, None),
        Some("linux-aarch64-appimage"),
    );
    assert_eq!(
        format_key("linux", "aarch64", true, Some(LinuxPackageFormat::Rpm)),
        Some("linux-aarch64-appimage"),
    );
    assert_eq!(
        format_key("linux", "aarch64", true, Some(LinuxPackageFormat::Deb)),
        Some("linux-aarch64-appimage"),
    );
}

#[test]
fn format_key_linux_x86_64_rpm() {
    assert_eq!(
        format_key("linux", "x86_64", false, Some(LinuxPackageFormat::Rpm)),
        Some("linux-x86_64-rpm"),
    );
}

#[test]
fn format_key_linux_aarch64_rpm() {
    assert_eq!(
        format_key("linux", "aarch64", false, Some(LinuxPackageFormat::Rpm)),
        Some("linux-aarch64-rpm"),
    );
}

#[test]
fn format_key_linux_x86_64_deb() {
    assert_eq!(
        format_key("linux", "x86_64", false, Some(LinuxPackageFormat::Deb)),
        Some("linux-x86_64-deb"),
    );
}

#[test]
fn format_key_linux_aarch64_deb() {
    assert_eq!(
        format_key("linux", "aarch64", false, Some(LinuxPackageFormat::Deb)),
        Some("linux-aarch64-deb"),
    );
}

#[test]
fn format_key_linux_x86_64_tarball() {
    assert_eq!(
        format_key("linux", "x86_64", false, None),
        Some("linux-x86_64-tarball"),
    );
}

#[test]
fn format_key_linux_aarch64_tarball() {
    assert_eq!(
        format_key("linux", "aarch64", false, None),
        Some("linux-aarch64-tarball"),
    );
}

#[test]
fn format_key_windows_x86_64() {
    assert_eq!(format_key("windows", "x86_64", false, None), Some("windows-x86_64-msi"));
    // `$APPIMAGE` and pkg detection are ignored on Windows — the wrapper
    // doesn't probe for them, but `format_key` itself should still
    // ignore them defensively.
    assert_eq!(format_key("windows", "x86_64", true, None), Some("windows-x86_64-msi"));
    assert_eq!(
        format_key("windows", "x86_64", false, Some(LinuxPackageFormat::Rpm)),
        Some("windows-x86_64-msi"),
    );
}

#[test]
fn format_key_windows_aarch64() {
    assert_eq!(
        format_key("windows", "aarch64", false, None),
        Some("windows-aarch64-msi"),
    );
    assert_eq!(format_key("windows", "aarch64", true, None), Some("windows-aarch64-msi"));
    assert_eq!(
        format_key("windows", "aarch64", false, Some(LinuxPackageFormat::Deb)),
        Some("windows-aarch64-msi"),
    );
}

#[test]
fn format_key_unsupported_platforms_return_none() {
    assert_eq!(format_key("macos", "x86_64", false, None), None);
    assert_eq!(format_key("macos", "aarch64", false, None), None);
    assert_eq!(format_key("freebsd", "x86_64", false, None), None);
    assert_eq!(format_key("linux", "riscv64", false, None), None);
    assert_eq!(format_key("linux", "armv7", false, None), None);
    assert_eq!(format_key("windows", "i686", false, None), None);
}

// ---------------------------------------------------------------------------
// cfg-gated end-to-end checks against `current_target_key()`. These
// validate that the wrapper plumbs the runtime probes through to
// `format_key` correctly on whatever arch CI is currently building for.
// The `format_key_*` tests above cover the arch combinations the host
// can't reach.
// ---------------------------------------------------------------------------

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn linux_x86_64_picks_appimage_when_env_set() {
    with_appimage_env(Some("/home/u/Apps/Melodia.AppImage"), || {
        assert_eq!(current_target_key(), Some("linux-x86_64-appimage"));
    });
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn linux_x86_64_picks_tarball_when_appimage_unset() {
    // Test binary lives under `target/debug/deps/`, which is not
    // owned by any RPM or .deb package — `linux_pkg::detect()` returns
    // `None` and the resolver falls through to the tarball.
    with_appimage_env(None, || {
        assert_eq!(current_target_key(), Some("linux-x86_64-tarball"));
    });
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn linux_aarch64_picks_appimage_when_env_set() {
    with_appimage_env(Some("/home/u/Apps/Melodia.AppImage"), || {
        assert_eq!(current_target_key(), Some("linux-aarch64-appimage"));
    });
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn linux_aarch64_picks_tarball_when_appimage_unset() {
    with_appimage_env(None, || {
        assert_eq!(current_target_key(), Some("linux-aarch64-tarball"));
    });
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn windows_x86_64_picks_msi() {
    assert_eq!(current_target_key(), Some("windows-x86_64-msi"));
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
fn windows_aarch64_picks_msi() {
    assert_eq!(current_target_key(), Some("windows-aarch64-msi"));
}

#[test]
fn install_target_uses_appimage_path_when_set() -> TestResult {
    let outcome: TestResult = std::cell::RefCell::new(Ok(())).into_inner();
    let result = std::cell::RefCell::new(outcome);
    with_appimage_env(Some("/home/u/Apps/Melodia.AppImage"), || {
        let r: TestResult = (|| {
            let target = install_target()?;
            if cfg!(target_os = "linux") {
                assert_eq!(target, PathBuf::from("/home/u/Apps/Melodia.AppImage"));
            } else {
                // On non-Linux platforms `$APPIMAGE` is ignored — falls
                // back to `current_exe()`.
                let cur = std::env::current_exe()?;
                assert_eq!(target, cur);
            }
            Ok(())
        })();
        *result.borrow_mut() = r;
    });
    result.into_inner()
}

#[test]
fn install_target_falls_back_to_current_exe_when_appimage_unset() -> TestResult {
    let result = std::cell::RefCell::new(Ok(()));
    with_appimage_env(None, || {
        let r: TestResult = (|| {
            let target = install_target()?;
            let cur = std::env::current_exe()?;
            assert_eq!(target, cur);
            Ok(())
        })();
        *result.borrow_mut() = r;
    });
    result.into_inner()
}

#[test]
fn install_target_ignores_empty_appimage_var() -> TestResult {
    let result = std::cell::RefCell::new(Ok(()));
    with_appimage_env(Some(""), || {
        let r: TestResult = (|| {
            let target = install_target()?;
            let cur = std::env::current_exe()?;
            assert_eq!(target, cur);
            Ok(())
        })();
        *result.borrow_mut() = r;
    });
    result.into_inner()
}
