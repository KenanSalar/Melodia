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
//! ## The orchestrator owns the whole row
//!
//! Suppression leaves the orchestrator as the **only** writer of the edited track's
//! row, and the event it drops did more than copy tags across: it ran
//! `update_track_metadata`, which also rewrites `file_hash`, `file_size` and
//! `date_modified`. None of those three are tag columns, and **all three change on
//! every tag write**, because rewriting a file's tags rewrites its bytes.
//!
//! The orchestrator already covers them — it re-extracts with
//! [`metadata::extract_metadata`](super::metadata::extract_metadata) after the
//! write, which `stat`s the file, BLAKE3-hashes it and re-parses the tags, and
//! feeds that whole `ExtractedMetadata` to the same `update_track_metadata`. So
//! suppression is not skipping that work; it is skipping the watcher's **second,
//! redundant copy** of it (plus a second `library_changed_tx` bump) a few seconds
//! later.
//!
//! What must never happen is "optimizing" the orchestrator into a hand-built UPDATE
//! from the tag values it already knows. It would still have to `stat` for
//! `date_modified`, and a fresh `date_modified` beside a stale `file_hash` is the
//! one state the boot-time reconcile cannot repair:
//! [`scanner::track_is_current`](super::scanner::track_is_current) compares the
//! stored size and mtime and would read the row as current forever, leaving a
//! permanently wrong hash behind moved-file detection and M3U8 playlist
//! re-matching. (`retroactive_hash` won't rescue it either: it backfills *missing*
//! hashes, not stale ones.)
//!
//! ## Mark with the DB `file_path`, never a picker path
//!
//! The set keys on **exact [`PathBuf`] equality** — it canonicalizes nothing. That
//! is sound only because both sides of the comparison descend from the same
//! canonicalized root: library folders are canonicalized when added, the watcher is
//! started on those exact paths, notify builds every event path from its watch
//! root, and the scanner writes `tracks.file_path` from a walk of that same root.
//!
//! Feed [`mark`](SelfWrites::mark) a path from anywhere else — an `rfd` picker, a
//! user-typed string, a relative path — and it simply won't match the event, so
//! suppression becomes a **silent no-op**: no error, no log, just the redundant
//! re-ingest quietly coming back. Mark with what `get_track_paths_by_ids` returned.
//!
//! ## Accepted trades
//!
//! - **An external edit to the same file inside the TTL window is swallowed.**
//!   Someone retagging in Kid3 in the same half-minute we wrote that file loses
//!   the live update; the boot-time `reconcile_watched_folders` catches it on the
//!   next launch (`track_is_current` compares the stored size and mtime).
//! - **Any other event kind for a marked path in the same batch strands its
//!   mark.** The processor's dedup pass keeps one event per path, and every other
//!   kind outranks `Modified` — `Created(p)` and `Renamed{to: p}` absorb it, and a
//!   later `Removed(p)` replaces it. So the `Modified` never reaches the filter,
//!   never consumes the entry, and the entry then shadows a genuine external edit
//!   for the rest of its TTL. It takes an external create/rename/delete racing our
//!   own write on the same path inside one 500 ms batch, and it self-heals at the
//!   next boot reconcile. Cheaper to accept than to teach dedup about a set it
//!   otherwise knows nothing of.
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

/// Never sweep below this many entries. A sweep is `O(n)`, and below this the map
/// is small enough that carrying a few expired entries costs nothing worth paying
/// for — they are inert either way, since [`SelfWrites::take_recent_at`] checks the
/// TTL of the entry it removes rather than trusting the sweep to have dropped it.
const MIN_SWEEP_AT: usize = 256;

struct Inner {
    map: HashMap<PathBuf, Instant>,
    /// Sweep once the map passes this many entries, then re-arm from what
    /// survived.
    ///
    /// A flat `if len >= THRESHOLD { retain }` would re-sweep on *every* call once
    /// the map was large, making a batch of `n` marks `O(n²)`. Re-arming to twice
    /// the surviving length amortizes it: `n` marks sweep `O(log n)` times.
    ///
    /// [`sweep`] re-arms this with [`MIN_SWEEP_AT`] as its floor, and `Default`
    /// seeds it there, so it is never below that floor — which is what lets both
    /// call sites test it directly.
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

/// Paths this process wrote recently, each with the instant it was written.
///
/// Lives on `AppState` as `Arc<SelfWrites>`. See the module header.
#[derive(Default)]
pub struct SelfWrites {
    inner: Mutex<Inner>,
}

/// Drop every entry older than [`SELF_WRITE_TTL`] and re-arm the sweep threshold
/// from what survived.
///
/// `duration_since` saturates to zero rather than panicking, so a clock quirk can
/// only ever make an entry look *young*, not negative.
fn sweep(inner: &mut Inner, now: Instant) {
    inner
        .map
        .retain(|_, written| now.duration_since(*written) < SELF_WRITE_TTL);
    inner.sweep_at = inner.map.len().saturating_mul(2).max(MIN_SWEEP_AT);
}

/// Sweep only once the map has outgrown its re-armed threshold.
///
/// Both call sites go through this, so neither can degrade into a per-call `O(n)`
/// scan: a batch edit marks `n` paths and then takes `n` `Modified` events back,
/// and an ungated sweep on either side would walk the whole map `n` times.
fn maybe_sweep(inner: &mut Inner, now: Instant) {
    if inner.map.len() >= inner.sweep_at {
        sweep(inner, now);
    }
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
        let mut inner = self.inner.lock();
        for path in paths {
            inner.map.remove(path);
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
        let mut inner = self.inner.lock();
        // The read path only runs when a `Modified` event actually arrives. A batch
        // edit with folder watching off (or one whose writes all errored and went
        // un-`unmark`ed) fires no events at all, so the insert path has to be what
        // bounds the map — without this it would hold every marked path for the
        // rest of the session.
        maybe_sweep(&mut inner, at);
        inner.map.insert(path.to_path_buf(), at);
    }

    fn take_recent_at(&self, path: &Path, now: Instant) -> bool {
        let mut inner = self.inner.lock();
        maybe_sweep(&mut inner, now);
        // The sweep is gated, so it may not have run — this entry's own TTL has to
        // be checked here rather than relying on the sweep to have dropped it. An
        // expired mark that still reported recent would swallow a genuine external
        // edit, which is the one way this set must never be wrong. `remove` either
        // way: a lookup consumes the entry, expired or not.
        inner
            .map
            .remove(path)
            .is_some_and(|written| now.duration_since(written) < SELF_WRITE_TTL)
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.inner.lock().map.len()
    }
}

#[cfg(test)]
#[path = "tests/self_writes_tests.rs"]
mod tests;
