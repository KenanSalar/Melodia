//! Retires stored artwork no database row points at any more.
//!
//! Artwork is shared — one cover serves every track on the album — so a per-track delete must not
//! unlink the file, and a refcount would have to be exactly right across scan ingest, orphan
//! cleanup, both watcher delete paths, tag edits that replace a cover, and composite regeneration.
//! A reference sweep cannot undercount because it never counts, and it is the only thing that can
//! reach files already orphaned by every one of those paths before it existed.
//!
//! Two gates decide what goes, and both have to hold: the name has to be one
//! [`super::is_stored_name`] recognises, and nothing in the reference set may name it. They are
//! two calls rather than one because the *order* matters — see [`collect_candidates`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::is_stored_name;

/// How long a file is protected from the sweep purely for being new.
///
/// A tag edit or a scan worker can have written a cover whose row hasn't committed yet, which
/// would otherwise read as an orphan and leave the committing transaction pointing at nothing. An
/// hour is far past any write window and costs at most one extra scan before a real orphan goes.
pub const GRACE: Duration = Duration::from_hours(1);

/// What one sweep did, for the caller's log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    /// Files unlinked.
    pub deleted: u32,
    /// Bytes those files occupied.
    pub bytes: u64,
    /// Stored files left in place — still referenced, or still inside [`GRACE`].
    pub kept: u32,
    /// Stored files that could not be read or unlinked; left alone, retried next sweep.
    pub failed: u32,
}

/// A stored file only the reference set can still save.
pub struct Candidate {
    path: PathBuf,
    name: String,
    bytes: u64,
}

/// Every stored file in `dir` the two clock-and-name gates left standing, plus what they already
/// decided.
///
/// **Call this before reading the reference set, never after.** A scan writes to both under it,
/// and only this order fails safe: a row committed between the two is visible to the query, where
/// the reverse reads it as an orphan and unlinks a live cover. [`GRACE`] does not cover that case
/// — `store_image` leaves mtime alone when its content-addressed name already exists, so a cover
/// carried over from an earlier session is long past the window the moment a re-scan points a row
/// back at it.
///
/// **Blocking** — one directory listing plus a `stat` per stored file.
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
        // A name that isn't UTF-8 is not one we wrote, so it fails the predicate either way.
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
        if within_grace(&metadata, grace, now) {
            report.kept += 1;
            continue;
        }
        candidates.push(Candidate {
            path: entry.path(),
            name: name.to_owned(),
            bytes: metadata.len(),
        });
    }
    (candidates, report)
}

/// Deletes every candidate `referenced` does not name, folding into the report
/// [`collect_candidates`] started.
///
/// `referenced` holds bare filenames rather than paths, and is deliberately the union across
/// *all four* columns rather than the one column that points into the store: the names are content
/// hashes, so a cross-directory hit means the bytes are genuinely identical and keeping the file
/// is the harmless direction. Over-keeping is a leak the next sweep collects; over-deleting is a
/// cover gone until a re-scan.
///
/// **Blocking** — one unlink per orphan.
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
        match std::fs::remove_file(&candidate.path) {
            Ok(()) => {
                report.deleted += 1;
                report.bytes += candidate.bytes;
            }
            Err(e) => {
                report.failed += 1;
                log::warn!("Could not retire {}: {e}", candidate.path.display());
            }
        }
    }
    report
}

/// Whether the file is too young to sweep. An unreadable or future mtime counts as young, a clock
/// that can't be trusted being no reason to delete.
fn within_grace(metadata: &std::fs::Metadata, grace: Duration, now: SystemTime) -> bool {
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    now.duration_since(modified).is_ok_and(|age| age < grace) || modified > now
}

#[cfg(test)]
#[path = "tests/sweep_tests.rs"]
mod tests;
