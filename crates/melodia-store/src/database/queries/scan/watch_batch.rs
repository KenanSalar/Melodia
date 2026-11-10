//! A deduplicated batch of watcher events, applied to the track rows in one transaction.
//!
//! Metadata comes in already extracted: the tag reads and the rename's fallback `stat` happen
//! before the batch reaches here, so nothing in the transaction touches a file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Transaction;

use super::cue_units::forget_gone_sheet;
use super::{
    MoveCandidates, NameCache, SheetWriter, TrackFolder, delete_track_by_path, file_name_of,
    find_folder_for_path, insert_track, prune_orphans, resolve_track_context, roll_up_covers,
    track_exists_by_path, update_track_location, update_track_metadata,
};
use crate::database::queries::ingest;
use crate::database::queries::{excluded_files, stats};
use crate::database::{DbPool, sql};
use crate::media::cue::{CueChunk, SheetJob};
use crate::media::ingest::watcher::is_sheet_file;
use melodia_core::entities::scan::{ExtractedMetadata, ScannedFile};
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
    /// The cue sheet work the batch's events owe, settled before its files were read, with every
    /// image its jobs claim or give back.
    Sheets {
        cue: CueChunk,
        images: Vec<ScannedFile>,
    },
}

/// What one batch carries from event to event.
#[derive(Default, Clone)]
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
/// deletes apply last (`apply_order`), since a `Removed` applied first would hard-delete the row
/// the `Created` needs and land the move as a fresh insert without the user's rating, play count
/// and favourite. Dedup emits from a `HashMap`, so the sort is what decides; stable, so nothing
/// else moves.
pub async fn apply_watch_batch(
    db: &DbPool,
    mut changes: Vec<WatchedChange>,
) -> Result<(), AppError> {
    let mut batch = Batch {
        moved: moved_candidates(db, &changes).await,
        names: NameCache::for_chunk(changes.len()),
    };
    changes.sort_by_key(apply_order);
    db.transaction(move |tx| apply_changes(tx, &changes, &mut batch)).await
}

/// The sheets apply after the other changes, so a renamed image's rows sit at its new path for its
/// sheet to update, and before the deletes, so a moved image's rows are still there to take. A
/// removed sheet goes last, once a removed image's rows no longer name it.
fn apply_order(change: &WatchedChange) -> u8 {
    match change {
        WatchedChange::Created { .. }
        | WatchedChange::Modified { .. }
        | WatchedChange::Renamed { .. } => 0,
        WatchedChange::Sheets { .. } => 1,
        WatchedChange::Removed { path, .. } if is_sheet_file(path) => 3,
        WatchedChange::Removed { .. } => 2,
    }
}

fn apply_changes(
    tx: &Transaction<'_>,
    changes: &[WatchedChange],
    batch: &mut Batch,
) -> Result<(), AppError> {
    let is_bulk = ingest::is_bulk(changes.len());
    if is_bulk {
        stats::disable_stats_triggers(tx)?;
    }

    // Gates the sweeps below, which a batch of events for untracked files mustn't pay for.
    let mut changed: usize = 0;

    for change in changes {
        match apply_change(tx, change, batch) {
            Ok(wrote) => changed += usize::from(wrote),
            Err(e) => log_failed(change, &e),
        }
    }
    lift_gone(tx, changes);

    if is_bulk {
        // With nothing changed the counts are still right, and only the re-enable is owed.
        if changed > 0 {
            stats::recalculate_all_stats(tx)?;
        }
        stats::enable_stats_triggers(tx)?;
    }

    if changed > 0 {
        roll_up_covers(tx)?;
        prune_orphans(tx)?;
    }
    Ok(())
}

/// Whether a row was written.
fn apply_change(
    tx: &Transaction<'_>,
    change: &WatchedChange,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    match change {
        WatchedChange::Created { path, meta } => handle_created(tx, path, meta, batch),
        WatchedChange::Removed { path, .. } => handle_removed(tx, path),
        WatchedChange::Renamed { from, to, meta, date_modified } => {
            handle_renamed(tx, from, to, meta.as_ref(), date_modified.as_deref(), batch)
        }
        WatchedChange::Modified { path, meta } => handle_modified(tx, path, meta, batch),
        WatchedChange::Sheets { cue, images } => Ok(handle_sheets(tx, cue, images, batch)),
    }
}

fn log_failed(change: &WatchedChange, e: &AppError) {
    match change {
        WatchedChange::Created { path, .. } => {
            log::warn!("Failed to process created file {}: {}", path.display(), describe(e));
        }
        WatchedChange::Removed { path, .. } => {
            log::warn!("Failed to remove track {}: {}", path.display(), describe(e));
        }
        WatchedChange::Renamed { from, to, .. } => {
            log::warn!(
                "Failed to process rename {} -> {}: {}",
                from.display(),
                to.display(),
                describe(e)
            );
        }
        WatchedChange::Modified { path, .. } => {
            log::warn!("Failed to process modified file {}: {}", path.display(), describe(e));
        }
        // `handle_sheets` logs each job's failure as it rolls the job back.
        WatchedChange::Sheets { .. } => {}
    }
}

/// The rows and kept-out files carrying a created file's hash, and the sheet images carrying a
/// claimed image's, whose own file is gone from disk: moved, not duplicated. Read before the write
/// transaction opens; a failed lookup costs the batch its move detection and nothing else.
async fn moved_candidates(db: &DbPool, changes: &[WatchedChange]) -> MoveCandidates {
    let mut hashes: Vec<String> = Vec::new();
    let mut image_hashes: Vec<String> = Vec::new();
    for change in changes {
        match change {
            WatchedChange::Created { meta, .. } => hashes.push(meta.file_hash.clone()),
            WatchedChange::Sheets { cue, images } => {
                let claimed = images.iter().filter(|image| cue.claims(&image.path));
                image_hashes.extend(claimed.map(|image| image.metadata.file_hash.clone()));
            }
            _ => {}
        }
    }
    if hashes.is_empty() && image_hashes.is_empty() {
        return MoveCandidates::default();
    }

    let mut candidates = match read_candidates(db, hashes, image_hashes).await {
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

async fn read_candidates(
    db: &DbPool,
    hashes: Vec<String>,
    image_hashes: Vec<String>,
) -> Result<MoveCandidates, AppError> {
    db.read(move |conn| {
        let hashes: Vec<&str> = hashes.iter().map(String::as_str).collect();
        let image_hashes: Vec<&str> = image_hashes.iter().map(String::as_str).collect();
        MoveCandidates::read_on(conn, &hashes, &image_hashes)
    })
    .await
}

/// Whether a row was written, by an insert or a re-point. A path already holding rows, a split
/// image's included, writes none beside them.
fn handle_created(
    tx: &Transaction<'_>,
    path: &Path,
    meta: &ExtractedMetadata,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    if track_exists_by_path(tx, &path.to_string_lossy())? {
        return Ok(false);
    }
    create_track(tx, path, meta, batch)
}

/// [`handle_created`] past its guard, for a new file or an image its sheet gives back. A kept-out
/// file writes no row: it is restamped, or its exclusion follows it where it moved.
fn create_track(
    tx: &Transaction<'_>,
    path: &Path,
    meta: &ExtractedMetadata,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    let path_str = path.to_string_lossy().into_owned();

    if excluded_files::id_at_on(tx, &path_str)?.is_some() {
        excluded_files::restamp_on(tx, &[(path, meta)])?;
        log::debug!("Kept out of the library, restamped: {}", path.display());
        return Ok(false);
    }

    // A failed folder lookup leaves the entry for a later same-hash event. A row moved in from
    // outside the library keeps no links until its folder's next scan, which reads it as unstamped
    // (`LINKED_STAMP`).
    if let Some((existing_id, old_path)) = batch.moved.track_for(&meta.file_hash, path) {
        let Some(folder) = library_folder(tx, path, &path_str, "Moved")? else {
            return Ok(false);
        };
        let file_name = file_name_of(path);
        let repointed = update_track_location(
            tx,
            &old_path,
            &path_str,
            &file_name,
            folder.id(),
            meta.date_modified.as_deref(),
        )?;
        batch.moved.spend_track(&meta.file_hash, existing_id);
        if repointed {
            log::info!("Detected moved file: {old_path} -> {path_str}");
            return Ok(true);
        }
        // Deletes apply last, so a scan removed the row after the candidates were read.
    }

    if let Some((exclusion_id, old_path)) = batch.moved.take_exclusion(&meta.file_hash) {
        carry_kept_out(tx, exclusion_id, &old_path, &path_str)?;
        return Ok(false);
    }

    let Some(folder) = library_folder(tx, path, &path_str, "Created")? else {
        return Ok(false);
    };
    let ids = resolve_track_context(tx, folder, meta, &mut batch.names)?;

    let file_name = file_name_of(path);
    let now = melodia_core::utils::now_rfc3339();
    let _new_id = insert_track(tx, &path_str, &file_name, meta, &ids, &now, &mut batch.names)?;
    log::info!("Added new track: {path_str}");

    Ok(true)
}

/// Whether a row was deleted. A sheet's stamp row goes once no row names it.
fn handle_removed(tx: &Transaction<'_>, path: &Path) -> Result<bool, AppError> {
    if is_sheet_file(path) {
        let forgotten = forget_gone_sheet(tx, &path.to_string_lossy())?;
        if forgotten {
            log::info!("Forgot cue sheet: {}", path.display());
        }
        return Ok(forgotten);
    }
    let deleted = delete_track_by_path(tx, &path.to_string_lossy())?;
    if deleted {
        log::info!("Removed track: {}", path.display());
    } else {
        log::debug!("Track not in DB, skip remove: {}", path.display());
    }
    Ok(deleted)
}

/// Lifts the exclusions whose file is gone. It runs after the events, so a `Created` has carried
/// any that moved, and takes them together, deleting a folder being an event per file.
fn lift_gone(tx: &Transaction<'_>, changes: &[WatchedChange]) {
    let paths: Vec<_> = changes
        .iter()
        .filter_map(|change| match change {
            WatchedChange::Removed { path, gone: true } => Some(path.to_string_lossy()),
            _ => None,
        })
        .collect();
    let paths: Vec<&str> = paths.iter().map(AsRef::as_ref).collect();
    match excluded_files::lift_on(tx, &paths) {
        Ok(0) => {}
        Ok(lifted) => log::info!("Forgot {lifted} kept-out files gone from disk"),
        Err(e) => log::warn!("Failed to lift kept-out files: {}", describe(&e)),
    }
}

/// Whether a row was written. A rename onto a kept-out path from outside the library, a tagger's
/// save through a temp file, is a `Created` there and stays out.
fn handle_renamed(
    tx: &Transaction<'_>,
    from: &Path,
    to: &Path,
    meta: Option<&ExtractedMetadata>,
    date_modified: Option<&str>,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    let from_str = from.to_string_lossy().into_owned();
    let to_str = to.to_string_lossy().into_owned();

    if track_exists_by_path(tx, &from_str)? {
        let Some(folder) = library_folder(tx, to, &to_str, "Renamed")? else {
            return Ok(false);
        };

        let file_name = file_name_of(to);
        // Asked inside this transaction, so the rows are there.
        let _repointed =
            update_track_location(tx, &from_str, &to_str, &file_name, folder.id(), date_modified)?;
        // The row's file now sits where the kept-out one did.
        excluded_files::lift_on(tx, &[to_str.as_str()])?;
        log::info!("Renamed track: {from_str} -> {to_str}");
        return Ok(true);
    }
    if let Some(exclusion_id) = excluded_files::id_at_on(tx, &from_str)? {
        carry_kept_out(tx, exclusion_id, &from_str, &to_str)?;
        return Ok(false);
    }
    if let Some(meta) = meta {
        return handle_created(tx, to, meta, batch);
    }

    Ok(false)
}

/// Carries a kept-out file's exclusion to where it was renamed or moved. It lifts instead where the
/// new path is outside every library folder, or is a track's, the row keeping its history over the
/// file written onto it.
fn carry_kept_out(
    tx: &Transaction<'_>,
    exclusion_id: i64,
    from: &str,
    to: &str,
) -> Result<(), AppError> {
    let folder_id =
        if track_exists_by_path(tx, to)? { None } else { find_folder_for_path(tx, to)? };
    match folder_id {
        Some(folder_id) => {
            excluded_files::repoint_on(tx, exclusion_id, to, folder_id)?;
            log::info!("Kept-out file moved: {from} -> {to}");
        }
        None => {
            excluded_files::lift_on(tx, &[from])?;
        }
    }
    Ok(())
}

/// Whether a row was written.
fn handle_modified(
    tx: &Transaction<'_>,
    path: &Path,
    meta: &ExtractedMetadata,
    batch: &mut Batch,
) -> Result<bool, AppError> {
    let path_str = path.to_string_lossy().into_owned();

    if !track_exists_by_path(tx, &path_str)? {
        return handle_created(tx, path, meta, batch);
    }

    let Some(folder) = library_folder(tx, path, &path_str, "Modified")? else {
        return Ok(false);
    };
    let ids = resolve_track_context(tx, folder, meta, &mut batch.names)?;

    let updated = update_track_metadata(tx, &path_str, meta, &ids, &mut batch.names)?;
    if updated {
        log::info!("Updated metadata for: {path_str}");
    }
    Ok(updated)
}

/// Whether a row was written. Each job is a savepoint of its own, so a sheet failing leaves nothing
/// half-written, its stamp included, and the others apply. It runs on a copy of the batch, kept
/// only once the savepoint is: a rolled-back artist's cached id would name whichever artist next
/// takes its rowid.
fn handle_sheets(
    tx: &Transaction<'_>,
    cue: &CueChunk,
    images: &[ScannedFile],
    batch: &mut Batch,
) -> bool {
    let mut wrote = false;
    for job in cue.jobs() {
        let mut job_batch = batch.clone();
        match sql::in_savepoint(tx, || apply_sheet_job(tx, job, images, &mut job_batch)) {
            Ok(job_wrote) => {
                wrote |= job_wrote;
                *batch = job_batch;
            }
            Err(e) => {
                log::warn!("Failed to apply cue sheet {}: {}", job.path.display(), describe(&e));
            }
        }
    }
    wrote
}

/// Writes `job` under its sheet's library folder. Each image it gives back takes its whole-file
/// row first, for the job to carry the playlist places onto.
fn apply_sheet_job(
    tx: &Transaction<'_>,
    job: &SheetJob,
    images: &[ScannedFile],
    batch: &mut Batch,
) -> Result<bool, AppError> {
    let Some(folder) = library_folder(tx, &job.path, &job.path.to_string_lossy(), "Cue")? else {
        return Ok(false);
    };
    let mut wrote = false;
    for image in images.iter().filter(|image| job.releases.contains(&image.path)) {
        wrote |= create_track(tx, &image.path, &image.metadata, batch)?;
    }

    let now = melodia_core::utils::now_rfc3339();
    let mut sheets = SheetWriter::new(tx, images, &now, &mut batch.moved);
    sheets.apply(job, folder)?;
    Ok(wrote || sheets.into_writes().any())
}

/// The library folder holding a watched `path`, or `None`, logged under `context`, outside every
/// one.
fn library_folder(
    tx: &Transaction<'_>,
    path: &Path,
    path_str: &str,
    context: &str,
) -> Result<Option<TrackFolder>, AppError> {
    let folder_id = find_folder_for_path(tx, path_str)?;
    if folder_id.is_none() {
        log::debug!("{context} file not in any library folder, skipping: {}", path.display());
    }
    Ok(folder_id.map(TrackFolder::Library))
}

#[cfg(test)]
#[path = "../tests/watch_batch_tests.rs"]
mod tests;
