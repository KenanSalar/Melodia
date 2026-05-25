//! "Should the in-app updater UI be hidden?" check.
//!
//! Hides the in-app "Download & Install" button (and skips spawning
//! the daily check task in `src/main.rs:226-227`) when there's no
//! viable in-app update path for the current install. The in-app
//! updater is wired for:
//!
//!   - Per-user installs whose directory we can `rename(2)` directly:
//!     `AppImage`, the tarball + `install-linux.sh` path.
//!   - **Direct-download RPM/.deb installs** (`dnf` / `apt` package
//!     ownership detected via [`super::linux_pkg::detect`]). These go
//!     to `/usr/bin/melodia` — not user-writable — but
//!     [`super::install::download_and_install`] runs the verified
//!     package through `pkexec dnf install` / `pkexec apt install`
//!     instead of a raw `pkexec mv`, so the package manager DB stays
//!     consistent and dependencies are re-resolved.
//!   - **Per-machine Windows MSI installs to `C:\Program Files\`.**
//!     The install dir is SYSTEM-owned (no direct write), but the
//!     in-app updater downloads the next signed MSI and hands it to
//!     `msiexec /i` — UAC prompts, the `MajorUpgrade` element in
//!     `wix/main.wxs` replaces the running version, and Restart
//!     Manager handles the live binary swap.
//!     The current target key is the discriminator (`windows-*-msi`),
//!     mirroring the [`super::linux_pkg::detect`] escape hatch for
//!     root-owned RPM/.deb installs.
//!
//! What *still* reports as system-managed (hiding the in-app UI and
//! surfacing the "Updates managed by your package manager" hint in
//! `ui/views/settings/update-section.slint`):
//!
//!   - A hand-installed tarball dropped under a root-owned directory
//!     (`/opt/melodia/`) — file isn't owned by `rpm` or `dpkg`, no
//!     package manager can drive an update. The pkexec-mv path in
//!     `install.rs` exists for this case but the UI stays hidden to
//!     keep the happy path obvious.
//!   - A **portable extract on Windows** (e.g. user unzipped a
//!     `Melodia.exe` to their Desktop or a USB stick instead of
//!     running the MSI). The current target key still resolves to
//!     `windows-*-msi` (it's `cfg!`-derived from the build target, not
//!     the runtime path), so left unguarded the updater would happily
//!     download an MSI and `msiexec /i` it — orphaning the portable
//!     copy under a fresh `C:\Program Files\Melodia\bin\` install.
//!     The Windows arm of [`probe`] gates on
//!     [`is_under_program_files`] before the MSI escape hatch so this
//!     case falls through to "no clean update path" and the UI hides.
//!
//! Probe is keyed on `install_target().parent()`, **not**
//! `current_exe().parent()` — on `AppImage` runs the latter would
//! resolve to a temporary squashfs mount under `/tmp/.mount_…/usr/bin/`,
//! which is read-only and would wrongly mark a user-installed
//! `AppImage` as "system-managed".

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
