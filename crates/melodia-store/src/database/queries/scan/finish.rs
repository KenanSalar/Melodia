//! A folder scan's last write.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{delete_tracks_by_paths_batch, get_all_track_paths_for_folder};
use super::{prune_orphans, roll_up_covers};
use crate::database::DbPool;
use crate::database::queries::ingest::Ingested;
use crate::database::queries::stats;
use crate::media::ingest::scanner::MediaWalk;
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;

/// Orphan pruning, the cover roll-ups and the stats recalc, in one transaction.
///
/// `walk` is the folder's complete walk, the set its rows are checked against for files that
/// vanished. A stopped scan has none to hand over and purges nothing: every row its walk didn't
/// reach would read as an orphan. It still owes the rest, since on the bulk path each chunk
/// committed with the stats triggers dropped and only the recalc here brings the counts back.
pub async fn commit_scan(
    db: &DbPool,
    folder: &Folder,
    walk: Option<MediaWalk>,
    ingested: &Ingested,
) -> Result<(), AppError> {
    let mut tx = db.write().begin().await?;
    let orphans = match walk {
        Some(walk) => {
            let db_paths = get_all_track_paths_for_folder(&mut tx, folder.id).await?;
            orphans_of(db_paths, walk)
        }
        None => Vec::new(),
    };
    // On the bulk path a full recalc follows anyway, so a large orphan
    // purge shouldn't pay per-row delete triggers on top — drop them for
    // the delete. Small deltas keep the triggers on (per-row maintenance
    // is the whole point of the `!is_bulk` branch).
    let is_bulk = ingested.is_bulk();
    let bulk_orphan_purge = is_bulk && !orphans.is_empty();
    if bulk_orphan_purge {
        stats::disable_stats_triggers(&mut tx).await?;
    }
    if !orphans.is_empty() {
        log::info!("Removing {} orphaned tracks from folder {}", orphans.len(), folder.path);
        delete_tracks_by_paths_batch(&mut tx, &orphans).await?;
    }

    // No-op rescans (every file unchanged, no orphans, no inserts) skip the
    // artwork roll-ups and the full stats recalc, O(rows) sweeps that produce
    // identical values when nothing changed.
    let any_changes = ingested.any() || !orphans.is_empty();

    if any_changes {
        roll_up_covers(&mut tx).await?;
        // Purged orphan tracks can leave their album/artist/genre empty; sweep those.
        prune_orphans(&mut tx).await?;
    }
    // Small deltas (`!is_bulk`) never dropped the triggers, so per-row
    // maintenance already kept the stats correct — no recalc needed at all.
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

/// Rows whose file is no longer on disk. Compared against the full on-disk set, not what was
/// parsed: the incremental filter leaves out unchanged files that are still present, and
/// treating those as orphans would delete the whole library. A row under a path the walk
/// couldn't read is spared for the same reason, its file being as likely there as gone.
fn orphans_of(db_paths: Vec<String>, walk: MediaWalk) -> Vec<String> {
    let on_disk: HashSet<String> = walk.files.into_iter().map(into_string_lossy).collect();
    db_paths
        .into_iter()
        .filter(|path| !on_disk.contains(path))
        .filter(|path| !walk.unreadable.iter().any(|dir| Path::new(path).starts_with(dir)))
        .collect()
}

/// Takes the path's own buffer, copying only for the rare path that isn't valid UTF-8.
fn into_string_lossy(path: PathBuf) -> String {
    path.into_os_string().into_string().unwrap_or_else(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
#[path = "../tests/scan_finish_tests.rs"]
mod tests;
