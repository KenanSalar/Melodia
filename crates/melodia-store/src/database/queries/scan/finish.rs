//! A folder scan's last write.

use super::{delete_tracks_by_paths_batch, prune_orphans, roll_up_covers};
use crate::database::DbPool;
use crate::database::queries::ingest::Ingested;
use crate::database::queries::stats;
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;

/// Orphan pruning, the cover roll-ups and the stats recalc, in one transaction.
///
/// `orphans` are the rows the folder's complete walk didn't find, from
/// [`WalkIndex`](crate::media::ingest::scanner::WalkIndex). A stopped scan passes none: an orphan
/// may be a file moved to a path the stop left unread, and purging it loses the rating and play
/// count the next scan would have carried across. It still owes the rest, since on the bulk path
/// each chunk committed with the stats triggers dropped and only the recalc here brings the
/// counts back.
pub async fn commit_scan(
    db: &DbPool,
    folder: &Folder,
    orphans: &[String],
    ingested: &Ingested,
) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;
    // On the bulk path the recalc below covers the purge, so its deletes skip the triggers.
    let is_bulk = ingested.is_bulk();
    let bulk_orphan_purge = is_bulk && !orphans.is_empty();
    if bulk_orphan_purge {
        stats::disable_stats_triggers(&mut tx).await?;
    }
    if !orphans.is_empty() {
        log::info!("Removing {} orphaned tracks from folder {}", orphans.len(), folder.path);
        delete_tracks_by_paths_batch(&mut tx, orphans).await?;
    }

    // A rescan that changed nothing skips the sweeps, which would only rewrite the same values.
    let any_changes = ingested.any() || !orphans.is_empty();

    if any_changes {
        roll_up_covers(&mut tx).await?;
        prune_orphans(&mut tx).await?;
    }
    // A small delta kept its triggers, so its counts are already right.
    if is_bulk {
        if any_changes {
            stats::recalculate_all_stats(&mut tx).await?;
        }
        if bulk_orphan_purge {
            stats::enable_stats_triggers(&mut tx).await?;
        }
    }
    tx.commit().await?;

    if ingested.moved > 0 {
        log::info!("Detected {} moved/renamed files", ingested.moved);
    }
    if ingested.updated > 0 {
        log::info!("Updated metadata for {} changed files", ingested.updated);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/scan_finish_tests.rs"]
mod tests;
