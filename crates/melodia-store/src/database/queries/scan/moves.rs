//! Where a new path's bytes may have come from: a row, or a kept-out file, whose own path is gone.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::Connection;

use crate::database::queries::{excluded_files, track};
use melodia_core::error::AppError;

type ByHash = HashMap<String, Vec<(i64, String)>>;

/// Candidates by hash, each taken once so two same-hash files can't both claim one.
#[derive(Default)]
pub(crate) struct MoveCandidates {
    /// Every row per hash, lowest id first, as `(id, old path)`: any of a hash's twins can be the
    /// one that moved.
    pub(crate) tracks: ByHash,
    exclusions: ByHash,
}

impl MoveCandidates {
    /// Every row and kept-out file carrying one of `hashes`, whether or not its file is still there.
    pub(crate) fn read_on(conn: &Connection, hashes: &[&str]) -> Result<Self, AppError> {
        Ok(Self {
            tracks: track::rows_by_hash_on(conn, hashes)?,
            exclusions: excluded_files::by_hash_on(conn, hashes)?,
        })
    }

    pub(crate) fn old_paths(&self) -> Vec<String> {
        let candidates = self.tracks.values().chain(self.exclusions.values()).flatten();
        candidates.map(|(_, path)| path.clone()).collect()
    }

    /// Keeps the candidates whose file is gone: one still on disk is a copy the new file sits
    /// beside, not where it came from.
    pub(crate) fn retain_gone(&mut self, still_present: &HashSet<String>) {
        for candidates in [&mut self.tracks, &mut self.exclusions] {
            candidates.retain(|_, files| {
                files.retain(|(_, path)| !still_present.contains(path));
                !files.is_empty()
            });
        }
    }

    /// The row a new file at `new_path` with `hash`'s bytes moved from: the one sharing its file
    /// name, so twins moved together keep their own histories, else the lowest id.
    pub(crate) fn track_for(&self, hash: &str, new_path: &Path) -> Option<(i64, String)> {
        let rows = self.tracks.get(hash)?;
        let same_name =
            rows.iter().find(|(_, old)| Path::new(old).file_name() == new_path.file_name());
        same_name.or(rows.first()).cloned()
    }

    /// Spends the row `id`, so no later file with `hash`'s bytes re-points it too.
    pub(crate) fn spend_track(&mut self, hash: &str, id: i64) {
        let Some(rows) = self.tracks.get_mut(hash) else {
            return;
        };
        rows.retain(|(row_id, _)| *row_id != id);
        if rows.is_empty() {
            self.tracks.remove(hash);
        }
    }

    /// A kept-out file a new file with `hash`'s bytes moved from, as `(exclusion id, old path)`.
    pub(crate) fn take_exclusion(&mut self, hash: &str) -> Option<(i64, String)> {
        let files = self.exclusions.get_mut(hash)?;
        let taken = files.pop();
        if files.is_empty() {
            self.exclusions.remove(hash);
        }
        taken
    }
}
