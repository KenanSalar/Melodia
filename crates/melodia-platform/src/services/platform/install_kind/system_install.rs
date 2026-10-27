//! Whether there is no in-app update path, which hides Download & Install and skips
//! `tasks::updater_daily`.
//!
//! Three shapes have one: a per-user install whose directory we can `rename(2)` in, a
//! package-owned RPM/DEB that `pkexec dnf/apt install` keeps consistent, and a per-machine MSI that
//! `msiexec /i` replaces. Two don't: a tarball hand-dropped under a root-owned directory, and a
//! **portable extract on Windows**, whose target key is `cfg!`-derived and still reads
//! `windows-*-msi`, so the updater would install a fresh MSI and orphan the portable copy.
//!
//! Keyed on `install_target()`, not `current_exe()`: under an `AppImage` the latter is a read-only
//! mount in `/tmp`, which would mark every user install system-managed.

use std::sync::OnceLock;

use super::install_target;
use super::linux_pkg;
use super::probe::dir_is_writable;
use super::target::current_target_key;

/// The install can't move mid-process, and three boot callers would each pay the probe's round
/// trips.
static CACHED: OnceLock<bool> = OnceLock::new();

/// Tests skip the cache: they mutate `$APPIMAGE`, which moves `install_target()`, and one memoised
/// probe would poison every later test in the binary.
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
    // Ahead of the writability check, which a portable extract would otherwise pass.
    #[cfg(target_os = "windows")]
    {
        if !is_under_program_files(&target) {
            return true;
        }
    }
    if dir_is_writable(parent) {
        return false;
    }
    if linux_pkg::detect().is_some() {
        return false;
    }
    if matches!(current_target_key(), Some("windows-x86_64-msi" | "windows-aarch64-msi")) {
        return false;
    }
    true
}

/// Under a Program Files root, by all three `%ProgramFiles*%` variables, a prefix match needing no
/// canonicalisation or COM call. The fallbacks cover a stripped container with every one unset,
/// where assuming an installed copy beats marking a real one portable.
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
