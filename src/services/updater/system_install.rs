//! "Should the in-app updater UI be hidden?" check.
//!
//! Hides the in-app "Download & Install" button (and skips spawning
//! `tasks::updater_daily`) when there's no viable in-app update path. The
//! updater is wired for:
//!
//!   - Per-user installs whose directory we can `rename(2)` directly:
//!     `AppImage` and the tarball + `install-linux.sh` path.
//!   - **Direct-download RPM/.deb installs** ([`super::linux_pkg::detect`]).
//!     `/usr/bin/melodia` isn't user-writable, but
//!     [`super::install::download_and_install`] runs the verified package
//!     through `pkexec dnf/apt install` rather than a raw `pkexec mv`, so the
//!     package DB stays consistent and dependencies are re-resolved.
//!   - **Per-machine Windows MSI installs** under `C:\Program Files\`. The
//!     directory is SYSTEM-owned, but handing the next signed MSI to
//!     `msiexec /i` lets UAC, `wix/main.wxs`'s `MajorUpgrade` and Restart
//!     Manager do the swap. Keyed on the `windows-*-msi` target, mirroring the
//!     `linux_pkg` escape hatch.
//!
//! What still reports as system-managed (hiding the UI and surfacing the
//! "managed by your package manager" hint in
//! `melodia-ui/ui/views/settings/update-section.slint`):
//!
//!   - A tarball hand-dropped under a root-owned directory (`/opt/melodia/`) —
//!     no package manager owns the file, so none can drive the update. The
//!     `pkexec mv` path exists for it, but the UI stays hidden to keep the
//!     happy path obvious.
//!   - A **portable extract on Windows** (unzipped rather than MSI-installed).
//!     The target key is `cfg!`-derived from the build target, not the runtime
//!     path, so it still resolves to `windows-*-msi`; left unguarded the
//!     updater would `msiexec /i` and orphan the portable copy under a fresh
//!     `C:\Program Files\` install. [`probe`]'s Windows arm gates on
//!     [`is_under_program_files`] before the MSI escape hatch.
//!
//! Probe is keyed on `install_target().parent()`, **not**
//! `current_exe().parent()` — under `AppImage` the latter resolves to a
//! read-only squashfs mount in `/tmp`, wrongly marking a user install as
//! system-managed.

use std::sync::OnceLock;

use super::install_target;
use super::linux_pkg;
use super::probe::dir_is_writable;
use super::target::current_target_key;

/// Cached `is_system_install()` result. The install location doesn't
/// move during a process's lifetime, so probing it more than once is
/// pure waste — three filesystem round-trips per call (open/create,
/// remove, and the parent-path resolve). Three callers ran the probe
/// at boot before this cache landed.
static CACHED: OnceLock<bool> = OnceLock::new();

/// Returns `true` when the directory holding the current executable
/// (or the `AppImage` file on `AppImage` runs) isn't writable by this
/// process. Memoised in [`CACHED`] — the first call probes, subsequent
/// calls hit the cache.
///
/// Tests skip the cache: sibling tests in this module mutate
/// `$APPIMAGE`, which feeds into `install_target()`'s parent-dir
/// resolution — caching the first probe's result would poison every
/// subsequent test in the same binary. Production builds always
/// memoise.
pub fn is_system_install() -> bool {
    if cfg!(test) {
        return probe();
    }
    *CACHED.get_or_init(probe)
}

fn probe() -> bool {
    let Ok(target) = install_target() else {
        return true;
    };
    let Some(parent) = target.parent() else {
        return true;
    };
    // Windows portable extract: the target key is `windows-*-msi`
    // regardless of where the binary lives, so an unqualified
    // `dir_is_writable` happy-path check would let a user-writable
    // location (Desktop, USB stick, …) pass through and trigger an
    // MSI install to `C:\Program Files\` that orphans the portable
    // copy. Treat anything outside a Program Files-class directory as
    // "no clean update path" *before* the writability check. Has no
    // effect on non-Windows builds.
    #[cfg(target_os = "windows")]
    {
        if !is_under_program_files(&target) {
            return true;
        }
    }
    // Happy path: directory is user-writable → in-place rename works,
    // not system-managed.
    if dir_is_writable(parent) {
        return false;
    }
    // Root-owned directory, but the binary is owned by a known package
    // manager — we can drive the upgrade through `pkexec dnf install` /
    // `pkexec apt install`. Surface the in-app updater.
    if linux_pkg::detect().is_some() {
        return false;
    }
    // Per-machine Windows MSI install (`Program Files\Melodia\bin\` is
    // SYSTEM-owned). The updater downloads the next signed `.msi` and
    // launches `msiexec /i` — UAC prompts, WiX `MajorUpgrade` replaces
    // the running version. Symmetric with the linux-pkg escape hatch
    // above: a writable directory isn't a prerequisite when the
    // install format owns the replace. (The portable-extract escape
    // hatch above has already filtered out hand-extracted Windows
    // binaries, so reaching this branch on Windows implies a real
    // Program Files install.)
    if matches!(
        current_target_key(),
        Some("windows-x86_64-msi" | "windows-aarch64-msi")
    ) {
        return false;
    }
    // Root-owned and not package-owned (hand-installed tarball under
    // `/opt/`, admin-deployed binary, etc.) — no clean update path.
    true
}

/// Returns `true` when `path` lives under one of the standard Windows
/// Program Files roots, indicating a Windows-Installer-managed install
/// rather than a portable extract. Checks all three
/// `%ProgramFiles*%` env vars so per-arch hosts (32-bit / 64-bit /
/// `ProgramW6432`) resolve correctly without hard-coding `C:\`.
///
/// Env-var lookup rather than `KNOWNFOLDERID` resolution because:
///   * we only need a prefix match (no canonicalisation),
///   * the env vars are set by the OS for every process on session
///     start and don't require a COM call,
///   * stub fallbacks (`C:\Program Files`) handle the vanishingly
///     rare case where every Program Files env var is unset (stripped
///     containers / customised builds) — better to assume Program
///     Files-class than to wrongly mark a real install as portable.
#[cfg(target_os = "windows")]
fn is_under_program_files(path: &std::path::Path) -> bool {
    use std::path::PathBuf;

    let mut roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    if roots.is_empty() {
        roots.push(PathBuf::from(r"C:\Program Files"));
        roots.push(PathBuf::from(r"C:\Program Files (x86)"));
    }
    roots.iter().any(|root| path.starts_with(root))
}

#[cfg(test)]
#[path = "tests/system_install_tests.rs"]
mod tests;
