//! "Should the in-app updater UI be hidden?" check.
//!
//! Hides the in-app "Download & Install" button and skips `tasks::updater_daily`
//! where there is no viable in-app update path. Three shapes have one: a per-user
//! install whose directory we can `rename(2)` (`AppImage`, tarball); a direct-download
//! RPM/deb, whose root-owned `/usr/bin/melodia` is still updatable because
//! [`super::install::download_and_install`] goes through `pkexec dnf/apt install`
//! rather than a raw `mv`, keeping the package DB consistent; and a per-machine
//! Windows MSI, where handing the next signed one to `msiexec /i` lets UAC, `WiX`'s
//! `MajorUpgrade` and Restart Manager do the swap.
//!
//! Two still report system-managed. A tarball hand-dropped under a root-owned
//! directory — the `pkexec mv` path exists for it, but the UI stays hidden to keep
//! the happy path obvious. And a **portable extract on Windows**, which is the
//! subtle one: the target key is `cfg!`-derived from the build target rather than
//! the runtime path, so it still resolves to `windows-*-msi`, and left unguarded the
//! updater would `msiexec /i` a fresh `C:\Program Files\` install and orphan the
//! portable copy.
//!
//! Keyed on `install_target().parent()`, **not** `current_exe().parent()` — under
//! `AppImage` the latter is a read-only squashfs mount in `/tmp`, which would mark
//! every user install system-managed.

use std::sync::OnceLock;

use super::install_target;
use super::linux_pkg;
use super::probe::dir_is_writable;
use super::target::current_target_key;

/// Cached [`is_system_install`] result. The install location can't move during a
/// process, and each probe costs three filesystem round-trips — open/create, remove
/// and the parent-path resolve — which three callers were paying at boot.
static CACHED: OnceLock<bool> = OnceLock::new();

/// Whether the directory holding the current executable — or the `AppImage` file on
/// an `AppImage` run — is unwritable by this process. Memoised in [`CACHED`].
///
/// Tests skip the cache: siblings here mutate `$APPIMAGE`, which feeds
/// `install_target()`'s parent-dir resolution, so one memoised probe would poison
/// every later test in the same binary.
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
    // Ahead of the writability check, since a portable extract still keys as
    // `windows-*-msi` and a user-writable location would otherwise pass the happy
    // path and trigger an MSI install that orphans it.
    #[cfg(target_os = "windows")]
    {
        if !is_under_program_files(&target) {
            return true;
        }
    }
    if dir_is_writable(parent) {
        return false;
    }
    // Root-owned but package-owned: the upgrade goes through `pkexec dnf/apt install`.
    if linux_pkg::detect().is_some() {
        return false;
    }
    // Per-machine MSI — `msiexec /i` owns the replace, so a writable directory is no
    // prerequisite. The guard above has already filtered out portable extracts.
    if matches!(current_target_key(), Some("windows-x86_64-msi" | "windows-aarch64-msi")) {
        return false;
    }
    // Root-owned and not package-owned: no clean update path.
    true
}

/// Whether `path` lives under a standard Windows Program Files root — an
/// Installer-managed install rather than a portable extract. All three
/// `%ProgramFiles*%` vars, so per-arch hosts resolve without hard-coding `C:\`.
///
/// Env vars rather than `KNOWNFOLDERID`: a prefix match needs no canonicalisation
/// and no COM call. The stub fallbacks cover a stripped container with every one
/// unset — assuming Program Files-class beats marking a real install portable.
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
