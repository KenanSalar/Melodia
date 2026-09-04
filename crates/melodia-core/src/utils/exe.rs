//! Where the running binary is, and whether it came out of the source tree.
//!
//! Both questions are asked before most of the app exists — [`is_dev_build`] decides the data
//! root, [`current_exe`] the respawn and install targets — so they sit at the bottom of the tree
//! rather than beside the updater that asks them most.

use std::path::{Path, PathBuf};

/// The running binary's path, with Linux's `" (deleted)"` marker resolved.
///
/// `std::env::current_exe()` is a bare `readlink("/proc/self/exe")` on Linux, and the kernel
/// appends that literal suffix once the dentry the process was exec'd from is unlinked. It
/// **resolves** rather than merely trimming, which is what makes it correct rather than cosmetic:
/// the replacement file sits at the stripped path, so respawning from it relaunches the binary the
/// user now has.
///
/// **The marker can only appear mid-session** — you cannot exec an unlinked path — which is what
/// sorts the callers; `.claude/rules/updater.md` walks that list and what their failure compounds
/// into.
///
/// Reach for this over `std::env::current_exe()` anywhere the path will be executed, installed to,
/// or written down. Inside the updater go through `install_target`, which answers the `$APPIMAGE`
/// question first.
pub fn current_exe() -> std::io::Result<PathBuf> {
    Ok(undeleted_exe(std::env::current_exe()?, Path::exists))
}

/// The pure half of [`current_exe`], with `exists` standing in for the filesystem.
///
/// The order of the three guards is the whole of it: the suffix test first, so the common case
/// costs no `stat`; a suffixed path that is itself a live file wins over its sibling, a file
/// genuinely named `… (deleted)` not being this bug; and anything unresolved comes back verbatim,
/// so the caller's error still reports what the kernel said.
///
/// Deliberately not `cfg`-gated to Linux — no other platform produces the marker, and the
/// live-file guard makes it inert where a path ends that way by coincidence. The strip goes
/// through `to_str`; a non-UTF-8 path comes back unchanged rather than reaching for the `unsafe`
/// `OsStr::from_encoded_bytes_unchecked`.
fn undeleted_exe(exe: PathBuf, exists: impl Fn(&Path) -> bool) -> PathBuf {
    const DELETED_MARKER: &str = " (deleted)";

    let Some(base) = exe.to_str().and_then(|p| p.strip_suffix(DELETED_MARKER)) else {
        return exe;
    };
    let base = PathBuf::from(base);
    if exists(&exe) || !exists(&base) {
        return exe;
    }
    base
}

/// Whether the running binary came out of the source tree rather than an install.
///
/// A `cfg!(debug_assertions)` alone would miss `cargo build --release`, which is a real way to run
/// this tree and produces a binary indistinguishable from a shipped one except for where it sits —
/// hence the second, path-shaped answer.
///
/// The raw `std::env::current_exe()` is deliberate where [`current_exe`] is otherwise the rule: the
/// `" (deleted)"` marker lands on the file name, which nothing below looks at.
#[must_use]
pub fn is_dev_build() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // .../target/<profile>/<binary>  →  parent = <profile>, grandparent = target
    exe.parent()
        .and_then(Path::parent)
        .is_some_and(|p| p.file_name().is_some_and(|n| n == "target"))
}

#[cfg(test)]
#[path = "tests/exe_tests.rs"]
mod tests;
