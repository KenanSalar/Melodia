//! The set of files *we* just rewrote, so the folder watcher can ignore its own
//! echo.
//!
//! [`tag_writer::apply_to_file`](super::tag_writer::apply_to_file) rewrites a
//! file's bytes in place, which inotify reports as `ModifyKind::Data` and the
//! watcher turns into a [`FileEvent::Modified`](super::watcher::FileEvent). Left
//! alone that event costs a full BLAKE3 re-hash + lofty re-parse + artwork
//! re-extract + a redundant `update_track_metadata` + a second
//! `library_changed_tx` bump — per edited file, ~3 s after the edit already
//! landed. It doesn't loop (a DB write fires no filesystem event), but a
//! 50-track batch pays for itself twice.
//!
//! So the tag orchestrator [`mark`](SelfWrites::mark)s each path immediately
//! before writing it, and the file-event processor drops any `Modified` event
//! whose path [`take_recent`](SelfWrites::take_recent)s true.
//!
//! ## Why per-file marking, not one mark per batch
//!
//! The TTL is relative to the event it is meant to catch. Marking a 500-file
//! batch up front would start the clock on file 500 long before that file is
//! written; marking each path just before its own write keeps every entry's TTL
//! aligned with the write that will generate the event. [`SELF_WRITE_TTL`]
//! comfortably covers notify's 2 s debounce plus the processor's 500 ms batching
//! window plus the write itself.
//!
//! ## Accepted trades
//!
//! - **An external edit to the same file inside the TTL window is swallowed.**
//!   Someone retagging in Kid3 in the same half-minute we wrote that file loses
//!   the live update; the boot-time `reconcile_watched_folders` catches it on the
//!   next launch (`track_is_current` compares the stored mtime byte-for-byte).
//! - **A debouncer overflow bypasses the set entirely — and that is correct, not
//!   a hole.** When notify's queue overflows, the batch arrives as
//!   `FileEvent::RescanNeeded` and short-circuits to a full reconcile before the
//!   filter is ever consulted. A rescan re-derives everything from disk, so our
//!   own writes are simply seen and re-ingested — wasted work, correct result.
//! - **[`take_recent`](SelfWrites::take_recent) consumes the entry.** If notify
//!   ever emits two `Modified` batches for a single write, the second isn't
//!   suppressed and degrades into one redundant re-ingest. The alternative — a
//!   non-consuming, TTL-only check — would swallow strictly more genuine external
//!   edits, which is the worse way to be wrong.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// How long a marked path stays suppressed. Long enough to cover notify's 2 s
/// debounce plus the processor's 500 ms batching window plus the write; short
/// enough that a leaked entry (a write that errored, so no event ever arrives)
/// stops shadowing external edits almost immediately.
pub const SELF_WRITE_TTL: Duration = Duration::from_secs(30);

/// Paths this process wrote recently, each with the instant it was written.
///
/// Lives on `AppState` as `Arc<SelfWrites>`. See the module header.
#[derive(Default)]
pub struct SelfWrites {
    inner: Mutex<HashMap<PathBuf, Instant>>,
}

impl SelfWrites {
    /// Record that we are about to write `path`. Call this *immediately before*
    /// the write, per file — not once up front for a whole batch.
    pub fn mark(&self, path: &Path) {
        self.mark_at(path, Instant::now());
    }

    /// Drop `paths` from the set so the watcher *does* re-ingest them.
    ///
    /// The safety valve for a write that succeeded on disk but whose database
    /// transaction then failed: without this, suppression would leave the row
    /// permanently stale until the next boot reconcile.
    pub fn unmark(&self, paths: &[PathBuf]) {
        let mut map = self.inner.lock();
        for path in paths {
            map.remove(path);
        }
    }

    /// Consume the entry for `path` and report whether we wrote it within the
    /// TTL. Expired entries are swept on the way through, so a write that
    /// errored — and therefore never fired an event to consume its entry — can't
    /// leak.
    pub fn take_recent(&self, path: &Path) -> bool {
        self.take_recent_at(path, Instant::now())
    }

    fn mark_at(&self, path: &Path, at: Instant) {
        self.inner.lock().insert(path.to_path_buf(), at);
    }

    fn take_recent_at(&self, path: &Path, now: Instant) -> bool {
        let mut map = self.inner.lock();
        // `duration_since` saturates to zero rather than panicking, so a clock
        // quirk can only ever make an entry look *young*, not negative.
        map.retain(|_, at| now.duration_since(*at) < SELF_WRITE_TTL);
        map.remove(path).is_some()
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.inner.lock().len()
    }
}

#[cfg(test)]
#[path = "tests/self_writes_tests.rs"]
mod tests;
