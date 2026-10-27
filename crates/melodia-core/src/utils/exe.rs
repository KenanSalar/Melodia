//! Where the running binary is, and whether it came out of the source tree: both asked before
//! most of the app exists, so they sit at the bottom of the tree.

use std::path::{Path, PathBuf};

/// The running binary's path with the `" (deleted)"` marker Linux appends to an unlinked dentry
/// resolved rather than trimmed, so a respawn relaunches the binary now at that path. The marker
/// only appears mid-session, which is what sorts the callers (`.claude/rules/updater.md`); inside
/// the updater go through `install_target`.
pub fn current_exe() -> std::io::Result<PathBuf> {
    Ok(undeleted_exe(std::env::current_exe()?, Path::exists))
}

/// The order of the guards is the whole of it: the suffix test first, so the common case costs no
/// `stat`; a live file genuinely named `… (deleted)` wins over its sibling; and anything
/// unresolved comes back verbatim for the caller's error. Not `cfg`-gated, the live-file guard
/// making it inert elsewhere, and a non-UTF-8 path comes back unchanged.
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

/// `debug_assertions` alone would miss `cargo build --release`, hence the `target/` grandparent.
/// The raw `std::env::current_exe()` is fine here: the marker lands on the file name, which this
/// never reads.
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
