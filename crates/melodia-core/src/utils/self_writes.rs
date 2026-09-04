//! The set of files *we* just rewrote, so the folder watcher can ignore its own echo.
//!
//! `melodia-store`'s `tag_writer::apply_to_file` rewrites a file's bytes in place, which comes
//! back as a `FileEvent::Modified` — a re-hash, a lofty re-parse, an artwork re-extract and a
//! redundant row write per file, seconds after the edit already landed. It can't loop, a DB write
//! firing no filesystem event, but a batch pays for itself twice. So the tag orchestrator
//! [`mark`](SelfWrites::mark)s each path immediately before writing it, and the file-event
//! processor drops any `Modified` whose path [`take_recent`](SelfWrites::take_recent)s true.
//!
//! **Mark per file, not per batch.** The TTL has to line up with the event it catches, and marking
//! a whole batch up front starts the clock on the last file long before it is written.
//!
//! **Mark with the DB `file_path`, never a picker path.** The set keys on exact [`PathBuf`]
//! equality and canonicalizes nothing, which is sound only because both sides descend from the
//! same canonicalized root. A path from anywhere else won't match, and suppression degrades to a
//! **silent no-op**.
//!
//! **Suppression makes the orchestrator the row's only writer**, so it must keep re-extracting
//! through `metadata::extract_metadata` after the write — the dropped event also refreshed
//! `file_hash`, `file_size` and `date_modified`, all three of which change on every tag write.
//! `.claude/rules/library-data.md` argues why that must never become a hand-built UPDATE.
//!
//! ## Accepted trades
//!
//! - **An external edit to the same file inside the TTL is swallowed**, caught by the next
//!   launch's `reconcile_watched_folders`.
//! - **Any other event kind for a marked path in the same batch strands its mark.** Dedup keeps
//!   one event per path and every other kind outranks `Modified`, so the entry is never consumed
//!   and shadows a genuine edit for the rest of its TTL. It needs an external create/rename/delete
//!   racing our own write on one path inside a single batch, and self-heals at the next boot
//!   reconcile.
//! - **A debouncer overflow bypasses the set, correctly** — the batch arrives as
//!   `FileEvent::RescanNeeded` and short-circuits to a full reconcile before the filter is
//!   consulted, and a rescan re-derives from disk.
//! - **[`take_recent`](SelfWrites::take_recent) consumes the entry**, so two `Modified` batches for
//!   one write leave the second unsuppressed. A non-consuming TTL-only check would swallow
//!   strictly more genuine external edits, which is the worse way to be wrong.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// How long a marked path stays suppressed. Long enough to cover notify's 2 s debounce plus the
/// processor's 500 ms batching window plus the write; short enough that a leaked entry — a write
/// that errored, so no event ever arrives — stops shadowing external edits almost immediately.
pub const SELF_WRITE_TTL: Duration = Duration::from_secs(30);

/// Never sweep below this many entries. A sweep is `O(n)`, and below this carrying a few expired
/// entries is free — inert either way, since [`SelfWrites::take_recent_at`] re-checks the TTL of
/// the entry it removes rather than trusting the sweep to have dropped it.
const MIN_SWEEP_AT: usize = 256;

struct Inner {
    map: HashMap<PathBuf, Instant>,
    /// Sweep once the map passes this many entries, then re-arm from what survived. A flat
    /// `if len >= THRESHOLD { retain }` would re-sweep on *every* call once the map was large,
    /// making `n` marks `O(n²)`; re-arming to twice the surviving length amortizes that to
    /// `O(log n)` sweeps.
    sweep_at: usize,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            sweep_at: MIN_SWEEP_AT,
        }
    }
}

/// Paths this process wrote recently, each with the instant it was written. Lives on `AppState` as
/// `Arc<SelfWrites>`; see the module header.
#[derive(Default)]
pub struct SelfWrites {
    inner: Mutex<Inner>,
}

/// Drop every entry older than [`SELF_WRITE_TTL`] and re-arm the sweep threshold from what
/// survived. `duration_since` saturates to zero rather than panicking, so a clock quirk can only
/// make an entry look *young*, never negative.
fn sweep(inner: &mut Inner, now: Instant) {
    inner.map.retain(|_, written| now.duration_since(*written) < SELF_WRITE_TTL);
    inner.sweep_at = inner.map.len().saturating_mul(2).max(MIN_SWEEP_AT);
}

/// Sweep only once the map has outgrown its re-armed threshold. Both call sites go through this: a
/// batch edit marks `n` paths then takes `n` events back, and an ungated sweep on either side
/// would walk the whole map `n` times.
fn maybe_sweep(inner: &mut Inner, now: Instant) {
    if inner.map.len() >= inner.sweep_at {
        sweep(inner, now);
    }
}

impl SelfWrites {
    /// Record that we are about to write `path`. Call this *immediately before* the write, per
    /// file — not once up front for a whole batch.
    pub fn mark(&self, path: &Path) {
        self.mark_at(path, Instant::now());
    }

    /// Drop `paths` from the set so the watcher *does* re-ingest them.
    ///
    /// The safety valve for a write that succeeded on disk but whose database transaction then
    /// failed — suppression would otherwise leave the row stale until the next boot reconcile.
    pub fn unmark(&self, paths: &[PathBuf]) {
        let mut inner = self.inner.lock();
        for path in paths {
            inner.map.remove(path);
        }
    }

    /// Consume the entry for `path` and report whether we wrote it within the TTL. Expired entries
    /// are swept on the way through, so a write that errored — and therefore never fired an event
    /// to consume its entry — can't leak.
    pub fn take_recent(&self, path: &Path) -> bool {
        self.take_recent_at(path, Instant::now())
    }

    fn mark_at(&self, path: &Path, at: Instant) {
        let mut inner = self.inner.lock();
        // The read path only runs when a `Modified` event arrives, and a batch edit with folder
        // watching off fires none — so the insert has to be what bounds the map, or it holds every
        // marked path for the rest of the session.
        maybe_sweep(&mut inner, at);
        inner.map.insert(path.to_path_buf(), at);
    }

    fn take_recent_at(&self, path: &Path, now: Instant) -> bool {
        let mut inner = self.inner.lock();
        maybe_sweep(&mut inner, now);
        // The sweep is gated and may not have run, so check this entry's own TTL rather than
        // trusting it was dropped: an expired mark reporting recent would swallow a genuine
        // external edit, the one way this set must never be wrong. `remove` either way — a lookup
        // consumes the entry, expired or not.
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
