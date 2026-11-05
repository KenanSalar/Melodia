//! A folder scan's last write.

use rusqlite::Transaction;

use super::{delete_tracks_by_paths_batch, prune_orphans, roll_up_covers};
use crate::database::DbPool;
use crate::database::queries::ingest::Ingested;
use crate::database::queries::{excluded_files, stats};
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;

/// What a folder's complete walk didn't find, from
/// [`WalkIndex`](crate::media::ingest::scanner::WalkIndex).
#[derive(Default)]
pub struct Unwalked {
    /// The folder's rows, which the commit purges.
    pub tracks: Vec<String>,
    /// The folder's kept-out files, whose exclusions the commit forgets.
    pub exclusions: Vec<String>,
}

impl Unwalked {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.exclusions.is_empty()
    }
}

/// Orphan pruning, the cover roll-ups and the stats recalc, in one transaction.
///
/// A stopped scan passes an empty [`Unwalked`]: an orphan may be a file moved to a path the stop
/// left unread, and purging it loses the rating and play count the next scan would have carried
/// across. It still owes the rest, since on the bulk path each chunk committed with the stats
/// triggers dropped and only the recalc here brings the counts back.
pub async fn commit_scan(
    db: &DbPool,
    folder: &Folder,
    unwalked: Unwalked,
    ingested: &Ingested,
) -> Result<(), AppError> {
    let ingested = *ingested;
    let (removed, forgotten) =
        db.transaction(move |tx| finish_on(tx, &unwalked, &ingested)).await?;

    if removed > 0 {
        log::info!("Removed {removed} orphaned tracks from folder {}", folder.path);
    }
    if forgotten > 0 {
        log::info!("Forgot {forgotten} kept-out files gone from folder {}", folder.path);
    }
    if ingested.moved > 0 {
        log::info!("Detected {} moved/renamed files", ingested.moved);
    }
    if ingested.updated > 0 {
        log::info!("Updated metadata for {} changed files", ingested.updated);
    }
    Ok(())
}

/// [`commit_scan`]'s transaction, answering the rows it purged and the exclusions it forgot.
fn finish_on(
    tx: &Transaction<'_>,
    unwalked: &Unwalked,
    ingested: &Ingested,
) -> Result<(u64, u64), AppError> {
    // On the bulk path the recalc below covers the purge, so its deletes skip the triggers.
    let is_bulk = ingested.is_bulk();
    let bulk_orphan_purge = is_bulk && !unwalked.tracks.is_empty();
    if bulk_orphan_purge {
        stats::disable_stats_triggers(tx)?;
    }
    // A row this scan re-pointed as moved has left its orphaned path, so only the delete counts.
    let removed = delete_tracks_by_paths_batch(tx, &unwalked.tracks)?;
    let exclusions: Vec<&str> = unwalked.exclusions.iter().map(String::as_str).collect();
    let forgotten = excluded_files::lift_on(tx, &exclusions)?;

    // A rescan that changed nothing skips the sweeps, which would only rewrite the same values.
    let any_changes = ingested.any() || removed > 0;

    if any_changes {
        roll_up_covers(tx)?;
        prune_orphans(tx)?;
    }
    // A small delta kept its triggers, so its counts are already right.
    if is_bulk {
        if any_changes {
            stats::recalculate_all_stats(tx)?;
        }
        if bulk_orphan_purge {
            stats::enable_stats_triggers(tx)?;
        }
    }
    Ok((removed, forgotten))
}

#[cfg(test)]
#[path = "../tests/scan_finish_tests.rs"]
mod tests;
