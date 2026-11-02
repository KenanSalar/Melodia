//! Where a new path's bytes may have come from: a row, or a kept-out file, whose own path is gone.

use std::collections::{HashMap, HashSet};

use sqlx::sqlite::SqliteConnection;

use crate::database::queries::{excluded_files, track};
use melodia_core::error::AppError;

/// Candidates by hash, each taken once so two same-hash files can't both claim one.
#[derive(Default)]
pub(crate) struct MoveCandidates {
    /// The lowest-id row per hash, `(id, old path)`, which a moved file re-points.
    pub(crate) tracks: HashMap<String, (i64, String)>,
    exclusions: HashMap<String, Vec<(i64, String)>>,
}

impl MoveCandidates {
    /// Every row and kept-out file carrying one of `hashes`, whether or not its file is still there.
    pub(crate) async fn read_on(
        conn: &mut SqliteConnection,
        hashes: &[&str],
    ) -> Result<Self, AppError> {
        Ok(Self {
            tracks: track::lowest_id_by_hash_on(conn, hashes).await?,
            exclusions: excluded_files::by_hash_on(conn, hashes).await?,
        })
    }

    pub(crate) fn old_paths(&self) -> Vec<String> {
        let tracks = self.tracks.values().map(|(_, path)| path);
        let exclusions = self.exclusions.values().flatten().map(|(_, path)| path);
        tracks.chain(exclusions).cloned().collect()
    }

    /// Keeps the candidates whose file is gone: one still on disk is a copy the new file sits
    /// beside, not where it came from.
    pub(crate) fn retain_gone(&mut self, still_present: &HashSet<String>) {
        self.tracks.retain(|_, (_, path)| !still_present.contains(path));
        self.exclusions.retain(|_, files| {
            files.retain(|(_, path)| !still_present.contains(path));
            !files.is_empty()
        });
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
