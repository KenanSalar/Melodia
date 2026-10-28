//! The files we just rewrote, so the folder watcher can ignore its own echo: a tag write renames
//! a copy over the track, which comes back as a `Created`, and re-ingesting it would pay for the
//! batch twice.
//!
//! Mark per file immediately before its write, since the TTL has to line up with the event, and
//! with the DB `file_path`: the set keys on exact [`PathBuf`] equality, so a picker path silently
//! suppresses nothing. Suppression leaves the tag orchestrator the row's only writer, so it
//! re-extracts after the write rather than trusting the form, keeping hash and mtime in step.
//!
//! Accepted trades: an external edit inside the TTL is swallowed until the next boot reconcile,
//! and so is one shadowed by a mark a same-batch `Removed` or `Renamed` stranded. Taking an entry
//! consumes it, since a TTL-only check would swallow more genuine edits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Covers the watcher's debounce, the processor's batching and the write, and is short enough
/// that a leaked mark (a write that errored, so no event comes) soon stops shadowing edits.
pub const SELF_WRITE_TTL: Duration = Duration::from_secs(30);

/// A sweep is `O(n)`, and below this a few expired entries are inert: `take_recent_at` checks the
/// TTL of the entry it removes.
const MIN_SWEEP_AT: usize = 256;

struct Inner {
    map: HashMap<PathBuf, Instant>,
    /// Re-armed to twice what survives a sweep, so `n` marks cost `O(log n)` sweeps rather than
    /// one each once the map is large.
    sweep_at: usize,
}

impl Default for Inner {
    fn default() -> Self {
        Self { map: HashMap::new(), sweep_at: MIN_SWEEP_AT }
    }
}

#[derive(Default)]
pub struct SelfWrites {
    inner: Mutex<Inner>,
}

/// `duration_since` saturates, so a clock quirk can only make an entry look young.
fn sweep(inner: &mut Inner, now: Instant) {
    inner.map.retain(|_, written| now.duration_since(*written) < SELF_WRITE_TTL);
    inner.sweep_at = inner.map.len().saturating_mul(2).max(MIN_SWEEP_AT);
}

fn maybe_sweep(inner: &mut Inner, now: Instant) {
    if inner.map.len() >= inner.sweep_at {
        sweep(inner, now);
    }
}

impl SelfWrites {
    /// Immediately before the write, per file.
    pub fn mark(&self, path: &Path) {
        self.mark_at(path, Instant::now());
    }

    /// For a write that landed on disk but whose transaction failed, so the watcher re-ingests it.
    pub fn unmark(&self, paths: &[PathBuf]) {
        let mut inner = self.inner.lock();
        for path in paths {
            inner.map.remove(path);
        }
    }

    /// Consumes the entry, `true` where `path` was written within the TTL.
    pub fn take_recent(&self, path: &Path) -> bool {
        self.take_recent_at(path, Instant::now())
    }

    fn mark_at(&self, path: &Path, at: Instant) {
        let mut inner = self.inner.lock();
        // The insert bounds the map too: with folder watching off, no event ever reads it.
        maybe_sweep(&mut inner, at);
        inner.map.insert(path.to_path_buf(), at);
    }

    fn take_recent_at(&self, path: &Path, now: Instant) -> bool {
        let mut inner = self.inner.lock();
        maybe_sweep(&mut inner, now);
        // The sweep may not have run, so this entry's own TTL decides: an expired mark reporting
        // recent would swallow a genuine edit, the one way this set must never be wrong.
        inner.map.remove(path).is_some_and(|written| now.duration_since(written) < SELF_WRITE_TTL)
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.inner.lock().map.len()
    }
}

#[cfg(test)]
#[path = "tests/self_writes_tests.rs"]
mod tests;
