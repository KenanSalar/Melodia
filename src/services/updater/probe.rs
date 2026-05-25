//! Directory writability probe shared by `install` (staging-path
//! selection) and `system_install` (UI gating).
//!
//! Probes by attempting to `create_new` a uniquely-named file. Avoids
//! the `access(W_OK)` TOCTOU class of bugs (which checks the calling
//! UID's permission bits without verifying that *this* process can
//! actually write — wrong answer under `nosuid`, ACLs, immutable
//! flags, full-disk, read-only mounts, …).

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-process monotonic counter for probe filenames. Combined with
/// the PID so concurrent calls from different threads can't collide
/// on the same `create_new` (which would otherwise return a false
/// "not writable" for every caller after the first).
static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Returns `true` when the calling process can create files in `dir`.
/// One syscall round-trip on the happy path (open + unlink); a single
/// failed open on the negative path. `dir` is expected to exist —
/// callers always pass `install_target().parent()` which is resolved
/// from the binary's own location.
pub fn dir_is_writable(dir: &Path) -> bool {
    let seq = PROBE_SEQ.fetch_add(1, Ordering::Relaxed);
    let probe = dir.join(format!(
        ".melodia-write-probe-{}-{seq}",
        std::process::id()
    ));
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "tests/probe_tests.rs"]
mod tests;
