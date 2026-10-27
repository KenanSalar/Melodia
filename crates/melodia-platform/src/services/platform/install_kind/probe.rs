//! Directory writability, for `install`'s staging choice and `system_install`'s UI gate.
//!
//! By `create_new` rather than `access(W_OK)`, which checks permission bits and is wrong under
//! `nosuid`, ACLs, immutable flags, a full disk or a read-only mount.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// With the PID, so concurrent probes can't collide on one `create_new` and read as unwritable.
static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);

/// `dir` is expected to exist, callers passing `install_target().parent()`.
pub fn dir_is_writable(dir: &Path) -> bool {
    let seq = PROBE_SEQ.fetch_add(1, Ordering::Relaxed);
    let probe = dir.join(format!(".melodia-write-probe-{}-{seq}", std::process::id()));
    match std::fs::OpenOptions::new().create_new(true).write(true).open(&probe) {
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
