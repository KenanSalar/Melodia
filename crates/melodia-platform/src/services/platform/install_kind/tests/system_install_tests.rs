use crate::services::platform::install_kind::system_install::is_system_install;
use melodia_testkit::with_appimage_env;

// `is_system_install()` queries `install_target().parent()`. Since
// `install_target()` resolves to either `$APPIMAGE` (when set) or
// `current_exe()`, the test framework's binary path is what we end up
// probing here. `target/debug/deps/*` is always writable for the user
// running `cargo test`.
//
// On Linux: writable dir → "not system-managed" (in-place rename
// works). On Windows: the probe additionally requires the binary to
// live under a Program Files-class directory before claiming the MSI
// escape hatch, so `target\debug\deps\` correctly resolves to
// "system-managed" (portable-extract semantics — no clean update
// path). The two platforms are tested separately because the
// semantics genuinely differ, not because of CI-sandbox quirks.
//
// The "read-only parent dir" negative case is not exercised here —
// chmod-000 isn't portable, and the probe logic is one
// `OpenOptions::create_new` call that's verified end-to-end on
// RPM-installed binaries during release testing.

#[cfg(not(target_os = "windows"))]
#[test]
fn writable_install_dir_is_not_system_managed_on_linux() {
    // Clearing `$APPIMAGE` puts the probe on the test binary's
    // `current_exe()` parent (always writable under `target/debug/deps/`)
    // rather than whatever a sibling `target_tests.rs` case left behind.
    with_appimage_env(None, || {
        assert!(
            !is_system_install(),
            "cargo test's target/debug/deps/ must be writable — \
             is_system_install() returning true means the probe is broken"
        );
    });
}

/// Mirrors the Linux assertion but inverts the expectation: the test
/// binary lives under `target\debug\deps\` (not Program Files), which
/// matches the "portable extract" case the new
/// [`system_install::is_under_program_files`] guard targets — the
/// updater UI should hide so we don't `msiexec /i` over a hand-placed
/// `Melodia.exe`. The Linux variant above can't assert the same
/// because Linux has no analogue to "Program Files-class only" —
/// dir-writability is sufficient on its own there.
#[cfg(target_os = "windows")]
#[test]
fn portable_extract_on_windows_is_system_managed() {
    // `$APPIMAGE` is Linux-only but the probe consults it
    // unconditionally; clear it for hygiene so a sibling test that
    // forgot to restore it can't poison this assertion.
    with_appimage_env(None, || {
        assert!(
            is_system_install(),
            "Windows test binary under target\\debug\\deps\\ isn't under \
             Program Files — probe must report system-managed so the \
             updater UI hides instead of msiexec'ing over a portable extract"
        );
    });
}
