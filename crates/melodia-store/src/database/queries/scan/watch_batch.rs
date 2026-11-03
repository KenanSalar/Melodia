//! A deduplicated batch of watcher events, applied to the track rows in one transaction.
//!
//! Metadata comes in already extracted: the tag reads and the rename's fallback `stat` happen
//! before the batch reaches here, so nothing in the transaction touches a file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{
    MoveCandidates, NameCache, delete_track_by_path, find_folder_for_path, get_track_id_by_path,
    insert_track, prune_orphans, resolve_track_context, roll_up_covers, track_exists_by_path,
    update_track_location, update_track_metadata,
};
use crate::database::DbPool;
use crate::database::queries::ingest::BULK_THRESHOLD;
use crate::database::queries::{excluded_files, stats};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::{AppError, describe};

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
        /// The mtime a re-pointed row stores, `meta`'s wherever there is one.
        /// `update_track_location` writes the mtime but not the size or hash, so a fresher `stat`
        /// would sit beside the previous scan's size, and a size-preserving tag edit would read as
        /// current to `scanner::file_is_current` from then on. An older mtime only fails toward a
        /// re-parse.
        date_modified: Option<String>,
    },
    Removed {
        path: PathBuf,
        /// Whether the path was gone when the batch was read. Only then does a kept-out file's
        /// exclusion lift: a save that deletes and recreates its file can land the two halves in
        /// separate batches.
        gone: bool,
    },
}

/// What one batch carries from event to event.
#[derive(Default)]
struct Batch {
    /// The rows and kept-out files whose file vanished, where a `Created` with those bytes
    /// re-points rather than inserts.
    moved: MoveCandidates,
    /// One per batch: a folder drop lands a release at a time, every file naming the same artist
    /// and genre.
    names: NameCache,
}

/// Applies `changes` to the library, carrying a moved file's row across a delete and a create.
///
/// A move between filesystems arrives as a `Removed` plus a `Created`, never a rename. **Both
/// halves are needed**: the candidates are resolved before the write transaction opens, and the
/// deletes apply last, since a `Removed` applied first would hard-delete the row the `Created`
/// needs and land the move as a fresh insert without the user's rating, play count and favourite.
/// Dedup emits from a `HashMap`, so the sort is what decides; stable, so nothing else moves.
pub async fn apply_watch_batch(
    db: &DbPool,
    mut changes: Vec<WatchedChange>,
) -> Result<(), AppError> {
    let mut batch = Batch {
        moved: moved_candidates(db, &changes).await,
        names: NameCache::for_chunk(changes.len()),
    };
    changes.sort_by_key(|change| matches!(change, WatchedChange::Removed { .. }));

    let is_bulk = changes.len() > BULK_THRESHOLD;
    let mut tx = db.write().begin().await?;

    if is_bulk {
        stats::disable_stats_triggers(&mut tx).await?;
    }

    // Gates the sweeps below, which a batch of events for untracked files mustn't pay for.
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
            WatchedChange::Removed { path, .. } => match handle_removed(&mut tx, path).await {
                Ok(wrote) => changed += usize::from(wrote),
                Err(e) => {
                    log::warn!("Failed to remove track {}: {}", path.display(), describe(&e));
                }
            },
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
    lift_gone(&mut tx, &changes).await;

    if is_bulk {
        // With nothing changed the counts are still right, and only the re-enable is owed.
        if changed > 0 {
            stats::recalculate_all_stats(&mut tx).await?;
        }
        stats::enable_stats_triggers(&mut tx).await?;
    }

    if changed > 0 {
        roll_up_covers(&mut tx).await?;
        prune_orphans(&mut tx).await?;
    }
    tx.commit().await?;

    Ok(())
}

/// The lowest-id row, and the kept-out files, for each created file's hash whose own file is gone
/// from disk: moved, not duplicated. Read before the write transaction opens; a failed lookup
/// costs the batch its move detection and nothing else.
async fn moved_candidates(db: &DbPool, changes: &[WatchedChange]) -> MoveCandidates {
    let hashes: Vec<&str> = changes
        .iter()
        .filter_map(|change| match change {
            WatchedChange::Created { meta, .. } => Some(meta.file_hash.as_str()),
            _ => None,
        })
        .collect();
    if hashes.is_empty() {
        return MoveCandidates::default();
    }

    let mut candidates = match read_candidates(db, &hashes).await {
        Ok(candidates) => candidates,
        Err(e) => {
            log::warn!("Move detection skipped for this batch: {}", describe(&e));
            return MoveCandidates::default();
        }
    };
    tokio::task::spawn_blocking(move || {
        let still_present: HashSet<String> =
            candidates.old_paths().into_iter().filter(|path| Path::new(path).exists()).collect();
        candidates.retain_gone(&still_present);
        candidates
    })
    .await
    .unwrap_or_default()
}

async fn read_candidates(db: &DbPool, hashes: &[&str]) -> Result<MoveCandidates, AppError> {
    let mut conn = db.read().acquire().await?;
    MoveCandidates::read_on(&mut conn, hashes).await
}

fn file_name_owned(path: &Path) -> String {
    path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_owned()
}

/// Whether a row was written, by an insert or a re-point. A kept-out file writes none: it is
/// restamped, or its exclusion follows it where it moved.
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
    if excluded_files::id_at_on(tx, &path_str).await?.is_some() {
        excluded_files::restamp_on(tx, &[(path, meta)]).await?;
        log::debug!("Kept out of the library, restamped: {}", path.display());
        return Ok(false);
    }

    // A failed folder lookup leaves the entry for a later same-hash event.
    if let Some((existing_id, old_path)) = batch.moved.tracks.get(&meta.file_hash).cloned() {
        let Some(folder_id) = find_folder_for_path(tx, &path_str).await? else {
            log::debug!("Moved file not in any library folder, skipping: {}", path.display());
            return Ok(false);
        };
        let file_name = file_name_owned(path);
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
            batch.moved.tracks.remove(&meta.file_hash);
            log::info!("Detected moved file: {old_path} -> {path_str}");
            return Ok(true);
        }
        // Deletes apply last, so a scan removed the row after the candidates were read.
        batch.moved.tracks.remove(&meta.file_hash);
    }

    if let Some((exclusion_id, old_path)) = batch.moved.take_exclusion(&meta.file_hash) {
        match find_folder_for_path(tx, &path_str).await? {
            Some(folder_id) => {
                excluded_files::repoint_on(tx, exclusion_id, &path_str, folder_id).await?;
                log::info!("Kept-out file moved: {old_path} -> {path_str}");
            }
            None => {
                excluded_files::lift_on(tx, &[old_path.as_str()]).await?;
            }
        }
        return Ok(false);
    }

    let Some(folder_id) = library_folder(tx, path, &path_str, "Created").await? else {
        return Ok(false);
    };
    let ids = resolve_track_context(tx, folder_id, meta, &mut batch.names).await?;

    let file_name = file_name_owned(path);
    let now = melodia_core::utils::now_rfc3339();
    let _new_id =
        insert_track(tx, &path_str, &file_name, meta, &ids, &now, &mut batch.names).await?;
    log::info!("Added new track: {path_str}");

    Ok(true)
}

/// Whether a row was deleted.
async fn handle_removed(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
) -> Result<bool, AppError> {
    let deleted = delete_track_by_path(tx, &path.to_string_lossy()).await?;
    if deleted {
        log::info!("Removed track: {}", path.display());
    } else {
        log::debug!("Track not in DB, skip remove: {}", path.display());
    }
    Ok(deleted)
}

/// Lifts the exclusions whose file is gone. It runs after the events, so a `Created` has carried
/// any that moved, and takes them together, deleting a folder being an event per file.
async fn lift_gone(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, changes: &[WatchedChange]) {
    let paths: Vec<_> = changes
        .iter()
        .filter_map(|change| match change {
            WatchedChange::Removed { path, gone: true } => Some(path.to_string_lossy()),
            _ => None,
        })
        .collect();
    let paths: Vec<&str> = paths.iter().map(AsRef::as_ref).collect();
    match excluded_files::lift_on(tx, &paths).await {
        Ok(0) => {}
        Ok(lifted) => log::info!("Forgot {lifted} kept-out files gone from disk"),
        Err(e) => log::warn!("Failed to lift kept-out files: {}", describe(&e)),
    }
}

/// Whether a row was written. A rename onto a kept-out path from outside the library, a tagger's
/// save through a temp file, is a `Created` there and stays out.
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
        // Resolved inside this transaction, so the row is there.
        let _repointed =
            update_track_location(tx, track_id, &to_str, &file_name, folder_id, date_modified)
                .await?;
        // The row's file now sits where the kept-out one did.
        excluded_files::lift_on(tx, &[to_str.as_str()]).await?;
        log::info!("Renamed track: {from_str} -> {to_str}");
        return Ok(true);
    }
    if let Some(exclusion_id) = excluded_files::id_at_on(tx, &from_str).await? {
        rename_kept_out(tx, exclusion_id, &from_str, &to_str).await?;
        return Ok(false);
    }
    if let Some(meta) = meta {
        return handle_created(tx, to, meta, batch).await;
    }

    Ok(false)
}

/// Carries a kept-out file's exclusion to its new name. It lifts instead where the new name is
/// outside every library folder, or is a track's, the row keeping its history over the file
/// written onto it.
async fn rename_kept_out(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    exclusion_id: i64,
    from: &str,
    to: &str,
) -> Result<(), AppError> {
    let folder_id = if track_exists_by_path(tx, to).await? {
        None
    } else {
        find_folder_for_path(tx, to).await?
    };
    match folder_id {
        Some(folder_id) => {
            excluded_files::repoint_on(tx, exclusion_id, to, folder_id).await?;
            log::info!("Renamed kept-out file: {from} -> {to}");
        }
        None => {
            excluded_files::lift_on(tx, &[from]).await?;
        }
    }
    Ok(())
}

/// Whether a row was written.
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

    let Some(folder_id) = library_folder(tx, path, &path_str, "Modified").await? else {
        return Ok(false);
    };
    let ids = resolve_track_context(tx, folder_id, meta, &mut batch.names).await?;

    update_track_metadata(tx, &path_str, meta, &ids, &mut batch.names).await?;
    log::info!("Updated metadata for: {path_str}");

    Ok(true)
}

/// The library folder holding a watched `path`, or `None`, logged under `context`, outside every
/// one.
async fn library_folder(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    path_str: &str,
    context: &str,
) -> Result<Option<i64>, AppError> {
    let folder_id = find_folder_for_path(tx, path_str).await?;
    if folder_id.is_none() {
        log::debug!("{context} file not in any library folder, skipping: {}", path.display());
    }
    Ok(folder_id)
}

#[cfg(test)]
#[path = "../tests/watch_batch_tests.rs"]
mod tests;
