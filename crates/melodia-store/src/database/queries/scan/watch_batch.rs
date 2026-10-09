//! A deduplicated batch of watcher events, applied to the track rows in one transaction.
//!
//! Metadata comes in already extracted: the tag reads and the rename's fallback `stat` happen
//! before the batch reaches here, so nothing in the transaction touches a file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{
    NameCache, delete_track_by_path, find_folder_for_path, get_track_id_by_path, insert_track,
    prune_orphans, resolve_track_context, roll_up_covers, track_exists_by_path,
    update_track_location, update_track_metadata,
};
use crate::database::DbPool;
use crate::database::queries::{stats, track};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::{AppError, describe};

/// Batch size threshold above which stats triggers are disabled for bulk processing.
const BULK_THRESHOLD: usize = 20;

/// One watcher event, with what extraction read off the file.
pub enum WatchedChange {
    Created {
        path: PathBuf,
        meta: ExtractedMetadata,
    },
    Modified {
        path: PathBuf,
        meta: ExtractedMetadata,
    },
    Renamed {
        from: PathBuf,
        to: PathBuf,
        /// `None` where `to` could not be read, or vanished before the batch was extracted.
        meta: Option<ExtractedMetadata>,
        /// The mtime a re-pointed row stores. `meta`'s own wherever there is one, since
        /// `update_track_location` writes the mtime but not the size or hash: a fresher `stat`
        /// would land beside the previous scan's size, and an in-place tag edit that happened not
        /// to change the size would then read as current to `scanner::track_is_current` on every
        /// later scan. An older mtime only ever fails toward a re-parse.
        date_modified: Option<String>,
    },
    Removed(PathBuf),
}

/// What one batch carries from event to event.
#[derive(Default)]
struct Batch {
    /// Hash → `(id, old path)` of a row whose file has vanished: where a `Created` with those
    /// bytes re-points rather than inserts. An entry is consumed on a successful re-point, so two
    /// same-hash files in one batch can't both steal the one row.
    moved: HashMap<String, (i64, String)>,
    /// One per batch, not one per event: a folder drop lands a release at a time, so every file in
    /// it names the same artist and genre.
    names: NameCache,
}

/// Applies `changes` to the library, carrying a moved file's row across a delete and a create.
///
/// **Both halves of that are here, and both are needed.** A move between filesystems reaches the
/// watcher as a `Removed` plus a `Created` rather than a rename. The candidates are resolved
/// before the write transaction opens, so the deletes are applied last: a `Removed` applied first
/// would hard-delete the very row the matching `Created` needs, and the move would land as a fresh
/// insert that drops the user's rating, play count and favourite. Dedup emits from a `HashMap`, so
/// without the sort the two orders are a coin flip. Stable, so nothing else is reordered.
pub async fn apply_watch_batch(
    db: &DbPool,
    mut changes: Vec<WatchedChange>,
) -> Result<(), AppError> {
    let mut batch = Batch {
        moved: moved_candidates(db, &changes).await,
        names: NameCache::for_chunk(changes.len()),
    };
    changes.sort_by_key(|change| matches!(change, WatchedChange::Removed(_)));

    let is_bulk = changes.len() > BULK_THRESHOLD;
    let mut tx = db.write().begin().await?;

    if is_bulk {
        stats::disable_stats_triggers(&mut tx).await?;
    }

    // Rows actually inserted / re-pointed / updated / deleted this batch.
    // Gates the post-loop sweeps: a no-op batch (events for untracked
    // files, paths outside library folders) must not pay the cover roll-ups
    // or a stats recalc, which mirrors the `any_changes` gate on the scan
    // path (`commit_scan`).
    let mut changed: usize = 0;

    for change in &changes {
        match change {
            WatchedChange::Created { path, meta } => {
                match handle_created(&mut tx, path, meta, &mut batch).await {
                    Ok(wrote) => changed += usize::from(wrote),
                    Err(e) => log::warn!(
                        "Failed to process created file {}: {}",
                        path.display(),
                        describe(&e)
                    ),
                }
            }
            WatchedChange::Removed(path) => {
                let path_str = path.to_string_lossy();
                match delete_track_by_path(&mut tx, &path_str).await {
                    Ok(true) => {
                        changed += 1;
                        log::info!("Removed track: {}", path.display());
                    }
                    Ok(false) => log::debug!("Track not in DB, skip remove: {}", path.display()),
                    Err(e) => {
                        log::warn!("Failed to remove track {}: {}", path.display(), describe(&e));
                    }
                }
            }
            WatchedChange::Renamed { from, to, meta, date_modified } => {
                let renamed = handle_renamed(
                    &mut tx,
                    from,
                    to,
                    meta.as_ref(),
                    date_modified.as_deref(),
                    &mut batch,
                )
                .await;
                match renamed {
                    Ok(wrote) => changed += usize::from(wrote),
                    Err(e) => log::warn!(
                        "Failed to process rename {} -> {}: {}",
                        from.display(),
                        to.display(),
                        describe(&e)
                    ),
                }
            }
            WatchedChange::Modified { path, meta } => {
                match handle_modified(&mut tx, path, meta, &mut batch).await {
                    Ok(wrote) => changed += usize::from(wrote),
                    Err(e) => log::warn!(
                        "Failed to process modified file {}: {}",
                        path.display(),
                        describe(&e)
                    ),
                }
            }
        }
    }

    if is_bulk {
        // Triggers were off during the loop; with zero changes the
        // denormalized stats are still correct, so only the re-enable is
        // unconditional.
        if changed > 0 {
            stats::recalculate_all_stats(&mut tx).await?;
        }
        stats::enable_stats_triggers(&mut tx).await?;
    }

    if changed > 0 {
        roll_up_covers(&mut tx).await?;
        // A deleted file can empty its album/artist/genre; prune the stranded rows.
        prune_orphans(&mut tx).await?;
    }
    tx.commit().await?;

    Ok(())
}

/// The lowest-id row for each created file's hash whose own file is gone from disk: genuinely
/// moved, not duplicated. One chunked query off the read pool and one `stat` pass off the runtime,
/// both before the write transaction opens.
///
/// A lookup that fails costs the batch its move detection and nothing else, so it is logged and
/// the batch goes on.
async fn moved_candidates(
    db: &DbPool,
    changes: &[WatchedChange],
) -> HashMap<String, (i64, String)> {
    let hashes: Vec<&str> = changes
        .iter()
        .filter_map(|change| match change {
            WatchedChange::Created { meta, .. } => Some(meta.file_hash.as_str()),
            _ => None,
        })
        .collect();
    if hashes.is_empty() {
        return HashMap::new();
    }

    let mut by_hash = match lowest_ids(db, &hashes).await {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("Move detection skipped for this batch: {}", describe(&e));
            return HashMap::new();
        }
    };
    tokio::task::spawn_blocking(move || {
        by_hash.retain(|_, (_, path)| !Path::new(path.as_str()).exists());
        by_hash
    })
    .await
    .unwrap_or_default()
}

async fn lowest_ids(
    db: &DbPool,
    hashes: &[&str],
) -> Result<HashMap<String, (i64, String)>, AppError> {
    let mut conn = db.read().acquire().await?;
    track::lowest_id_by_hash_on(&mut conn, hashes).await
}

fn file_name_owned(path: &Path) -> String {
    path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_owned()
}

/// Returns `true` when a row was actually written (insert or moved-file
/// re-point) so the caller can gate the per-batch sweeps on real changes.
async fn handle_created(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    meta: &ExtractedMetadata,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    let path_str = path.to_string_lossy().into_owned();

    if track_exists_by_path(tx, &path_str).await? {
        return Ok(false);
    }

    // Move detection: same content hash + the previous owner's path is now
    // missing → re-point the existing row instead of inserting a new one.
    // The entry is consumed only on a successful re-point, so a failed folder
    // lookup leaves it available to a later same-hash event.
    if let Some((existing_id, old_path)) = batch.moved.get(&meta.file_hash).cloned() {
        let Some(folder_id) = find_folder_for_path(tx, &path_str).await? else {
            log::debug!("Moved file not in any library folder, skipping: {}", path.display());
            return Ok(false);
        };
        let file_name = file_name_owned(path);
        // `meta` was extracted from this very path, so its `date_modified` is the
        // mtime already in hand — re-deriving it would `stat` the file again and
        // could pair a fresh mtime with the size/hash of the earlier instant.
        let repointed = update_track_location(
            tx,
            existing_id,
            &path_str,
            &file_name,
            folder_id,
            meta.date_modified.as_deref(),
        )
        .await?;
        if repointed {
            batch.moved.remove(&meta.file_hash);
            log::info!("Detected moved file: {old_path} -> {path_str}");
            return Ok(true);
        }
        // 0 rows: the candidate is gone. Deletes are applied last, so it wasn't
        // one of this batch's; what's left is a scan committing a delete between
        // the pre-transaction candidate read and this write. Drop the dead entry
        // and fall through to a fresh insert.
        batch.moved.remove(&meta.file_hash);
    }

    let Some(ids) =
        resolve_track_context(tx, path, &path_str, meta, "Created", &mut batch.names).await?
    else {
        return Ok(false);
    };

    let file_name = file_name_owned(path);
    let now = melodia_core::utils::now_rfc3339();
    let _new_id =
        insert_track(tx, &path_str, &file_name, meta, &ids, &now, &mut batch.names).await?;
    log::info!("Added new track: {path_str}");

    Ok(true)
}

/// Returns `true` when a row was actually written. See [`handle_created`].
async fn handle_renamed(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    from: &Path,
    to: &Path,
    meta: Option<&ExtractedMetadata>,
    date_modified: Option<&str>,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    let from_str = from.to_string_lossy().into_owned();
    let to_str = to.to_string_lossy().into_owned();

    if let Some(track_id) = get_track_id_by_path(tx, &from_str).await? {
        let Some(folder_id) = find_folder_for_path(tx, &to_str).await? else {
            log::debug!("Renamed file not in any library folder, skipping: {}", to.display());
            return Ok(false);
        };

        let file_name = file_name_owned(to);
        // `track_id` was resolved by path inside this transaction, so the
        // row can't have vanished — the re-point bool is vacuously true.
        let _repointed =
            update_track_location(tx, track_id, &to_str, &file_name, folder_id, date_modified)
                .await?;
        log::info!("Renamed track: {from_str} -> {to_str}");
        return Ok(true);
    } else if let Some(meta) = meta {
        return handle_created(tx, to, meta, batch).await;
    }

    Ok(false)
}

/// Returns `true` when a row was actually written. See [`handle_created`].
async fn handle_modified(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    meta: &ExtractedMetadata,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    let path_str = path.to_string_lossy().into_owned();

    if !track_exists_by_path(tx, &path_str).await? {
        return handle_created(tx, path, meta, batch).await;
    }

    let Some(ids) =
        resolve_track_context(tx, path, &path_str, meta, "Modified", &mut batch.names).await?
    else {
        return Ok(false);
    };

    update_track_metadata(tx, &path_str, meta, &ids, &mut batch.names).await?;
    log::info!("Updated metadata for: {path_str}");

    Ok(true)
}

#[cfg(test)]
#[path = "../tests/watch_batch_tests.rs"]
mod tests;
