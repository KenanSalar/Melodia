//! Retires stored artwork no database row points at any more.
//!
//! By reference rather than by refcount. One cover serves a whole album, and a refcount would have
//! to stay exact across scan ingest, orphan cleanup, both watcher delete paths, cover-replacing tag
//! edits and composite regeneration; a sweep never counts, so it can't undercount.
//!
//! Two gates, both required: [`super::is_stored_name`] has to recognise the name, and nothing in
//! the reference set may name it. Two calls because the order matters ([`collect_candidates`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use melodia_core::error::describe;

use super::is_stored_name;

/// How long a new file is spared: a tag edit or scan worker may have written a cover whose row
/// hasn't committed yet. What it spares counts as [`SweepReport::deferred`].
pub const GRACE: Duration = Duration::from_hours(1);

/// What one sweep did, for the caller's log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    pub deleted: u32,
    /// Of the deleted files.
    pub bytes: u64,
    /// Still referenced.
    pub kept: u32,
    /// Unreferenced but inside the grace window, so owed a later sweep.
    pub deferred: u32,
    /// Could not be read or unlinked; retried next sweep.
    pub failed: u32,
}

/// A stored file only the reference set can still save.
pub struct Candidate {
    path: PathBuf,
    name: String,
    bytes: u64,
    /// Spared even unreferenced, but still checked so the report can say a later sweep is owed.
    young: bool,
}

/// Every stored file in `dir` the name gate left standing, plus what it already decided.
///
/// **Call this before reading the reference set, never after.** A scan writes both meanwhile, and
/// only this order fails safe: a row committed in between is visible to the query, where the
/// reverse unlinks a live cover. [`GRACE`] doesn't cover it, since `store_image` leaves an existing
/// file's mtime alone and a re-scan can point a row back at a cover from an earlier session.
/// **Blocking.**
pub fn collect_candidates(
    dir: &Path,
    grace: Duration,
    now: SystemTime,
) -> (Vec<Candidate>, SweepReport) {
    let mut report = SweepReport::default();
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::warn!("Could not list the artwork store at {}", dir.display());
        return (Vec::new(), report);
    };

    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        // Not UTF-8, so not a name we wrote.
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !is_stored_name(name) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            report.failed += 1;
            continue;
        };
        candidates.push(Candidate {
            path: entry.path(),
            name: name.to_owned(),
            bytes: metadata.len(),
            young: within_grace(&metadata, grace, now),
        });
    }
    (candidates, report)
}

/// Narrows `candidates` to the files `names` lists, for a caller that knows which files it
/// released and owes the rest no decision.
pub fn named_in<S: std::hash::BuildHasher>(
    candidates: Vec<Candidate>,
    names: &HashSet<String, S>,
) -> Vec<Candidate> {
    candidates.into_iter().filter(|candidate| names.contains(&candidate.name)).collect()
}

/// Deletes every candidate `referenced` doesn't name, adding to [`collect_candidates`]' report.
///
/// `referenced` is bare filenames across *every* artwork column, not just this store's: the names
/// are content hashes, so a cross-directory hit is identical bytes and keeping it is the harmless
/// way to be wrong. Over-keeping leaks until the next sweep; over-deleting loses a cover until a
/// re-scan. **Blocking.**
pub fn retire<S: std::hash::BuildHasher>(
    candidates: Vec<Candidate>,
    referenced: &HashSet<String, S>,
    mut report: SweepReport,
) -> SweepReport {
    for candidate in candidates {
        if referenced.contains(&candidate.name) {
            report.kept += 1;
            continue;
        }
        if candidate.young {
            report.deferred += 1;
            continue;
        }
        match std::fs::remove_file(&candidate.path) {
            Ok(()) => {
                report.deleted += 1;
                report.bytes += candidate.bytes;
            }
            // A sweep running beside this one got there first.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                report.failed += 1;
                log::warn!("Could not retire {}: {}", candidate.path.display(), describe(&e));
            }
        }
    }
    report
}

/// An unreadable or future mtime counts as young, an untrustworthy clock being no reason to delete.
fn within_grace(metadata: &std::fs::Metadata, grace: Duration, now: SystemTime) -> bool {
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    now.duration_since(modified).is_ok_and(|age| age < grace) || modified > now
}

#[cfg(test)]
#[path = "tests/sweep_tests.rs"]
mod tests;
