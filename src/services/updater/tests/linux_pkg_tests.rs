// All tests in this file gate on `#[cfg(unix)]` or `#[cfg(target_os = "linux")]`
// — they exercise APPIMAGE / PATH env-mutation helpers and an executable-bit
// shim that only make sense on Unix. The env-resetting helpers are unused on
// Windows, so file-scope the whole module.
#![cfg(unix)]

use crate::services::updater::linux_pkg::{LinuxPackageFormat, detect, resolve_install_program};
use crate::test_support::{with_appimage_env, with_env_var};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Runs `body` with `$PATH` replaced by `value`. Local because this is the only
/// file that needs PATH isolation; `with_appimage_env` is shared, since
/// `target_tests.rs` overrides the same variable.
fn with_path_env<F: FnOnce()>(value: &str, body: F) {
    with_env_var("PATH", Some(value), body);
}

/// Creates an executable shim file at `path` with `0o755` permissions.
/// Returns the path for chaining.
#[cfg(unix)]
fn make_exec_shim(path: &std::path::Path) -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, "#!/bin/sh\n")?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

// The test binary lives at `target/debug/deps/<name>`. That path is
// inside the cargo build tree, never owned by an RPM or .deb package.
// `detect()` should return `None`.
#[test]
#[cfg(target_os = "linux")]
fn detect_returns_none_for_cargo_test_binary() {
    with_appimage_env(None, || {
        assert_eq!(
            detect(),
            None,
            "test binary under target/debug/deps/ is not package-owned — \
             detect() returning Some means probe wrongly matched"
        );
    });
}

#[test]
#[cfg(target_os = "linux")]
fn detect_returns_none_under_appimage() {
    with_appimage_env(Some("/home/u/Apps/Melodia.AppImage"), || {
        assert_eq!(
            detect(),
            None,
            "AppImage runs short-circuit to None (squashfs paths aren't package-owned)"
        );
    });
}

#[test]
fn resolve_install_program_returns_none_with_empty_path() {
    with_path_env("", || {
        assert_eq!(resolve_install_program(LinuxPackageFormat::Rpm), None);
        assert_eq!(resolve_install_program(LinuxPackageFormat::Deb), None);
    });
}

#[test]
#[cfg(unix)]
fn resolve_install_program_picks_dnf5_over_dnf() -> TestResult {
    let tmp = tempfile::tempdir()?;
    make_exec_shim(&tmp.path().join("dnf5"))?;
    make_exec_shim(&tmp.path().join("dnf"))?;
    let path = tmp.path().to_string_lossy().into_owned();
    with_path_env(&path, || {
        assert_eq!(resolve_install_program(LinuxPackageFormat::Rpm), Some("dnf5"));
    });
    Ok(())
}

#[test]
#[cfg(unix)]
fn resolve_install_program_falls_back_to_dnf_when_dnf5_missing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    make_exec_shim(&tmp.path().join("dnf"))?;
    let path = tmp.path().to_string_lossy().into_owned();
    with_path_env(&path, || {
        assert_eq!(resolve_install_program(LinuxPackageFormat::Rpm), Some("dnf"));
    });
    Ok(())
}

#[test]
#[cfg(unix)]
fn resolve_install_program_picks_apt_over_apt_get() -> TestResult {
    let tmp = tempfile::tempdir()?;
    make_exec_shim(&tmp.path().join("apt"))?;
    make_exec_shim(&tmp.path().join("apt-get"))?;
    let path = tmp.path().to_string_lossy().into_owned();
    with_path_env(&path, || {
        assert_eq!(resolve_install_program(LinuxPackageFormat::Deb), Some("apt"));
    });
    Ok(())
}

#[test]
#[cfg(unix)]
fn resolve_install_program_falls_back_to_apt_get_when_apt_missing() -> TestResult {
    let tmp = tempfile::tempdir()?;
    make_exec_shim(&tmp.path().join("apt-get"))?;
    let path = tmp.path().to_string_lossy().into_owned();
    with_path_env(&path, || {
        assert_eq!(resolve_install_program(LinuxPackageFormat::Deb), Some("apt-get"));
    });
    Ok(())
}

/// Regression: a non-executable file in `$PATH` must NOT satisfy the
/// PATH lookup. The previous `Path::is_file()` check (before
/// `is_executable_file` landed) would wrongly accept a `0o644` file
/// named `dnf` and let the subsequent `pkexec dnf install` spawn fail
/// with a confusing exec error. Pinning both formats so a future
/// "simplification" back to `is_file()` fails CI.
#[test]
#[cfg(unix)]
fn resolve_install_program_rejects_non_executable_path_entry() -> TestResult {
    let tmp = tempfile::tempdir()?;
    // 0o644 by default — readable but not executable.
    std::fs::write(tmp.path().join("dnf"), "#!/bin/sh\n")?;
    std::fs::write(tmp.path().join("apt"), "#!/bin/sh\n")?;
    let path = tmp.path().to_string_lossy().into_owned();
    with_path_env(&path, || {
        assert_eq!(
            resolve_install_program(LinuxPackageFormat::Rpm),
            None,
            "0o644 `dnf` in PATH must be ignored — not executable"
        );
        assert_eq!(
            resolve_install_program(LinuxPackageFormat::Deb),
            None,
            "0o644 `apt` in PATH must be ignored — not executable"
        );
    });
    Ok(())
}
