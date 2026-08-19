//! Apply a deduplicated batch of file events to the database: extract
//! metadata for created/modified/renamed paths (outside any transaction),
//! then upsert / relocate / delete the corresponding track rows.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::Paths;
use crate::database::DbPool;
use crate::database::queries;
use crate::error::AppResult;
use crate::media::artwork::CoverCache;
use crate::media::metadata::{ExtractedMetadata, extract_date_modified, extract_or_filename_row};
use crate::media::watcher::FileEvent;

/// Batch size threshold above which stats triggers are disabled for bulk processing.
const BULK_THRESHOLD: usize = 20;

/// Extract metadata for all paths that need it (Created, Modified, Renamed-to).
/// Runs outside any DB transaction to avoid holding the write lock during I/O.
async fn extract_metadata_batch(
    paths: &Paths,
    cover_cache: &CoverCache,
    events: &[FileEvent],
) -> HashMap<PathBuf, ExtractedMetadata> {
    let artwork_dir = paths.artwork_dir.clone();

    let mut seen = HashSet::with_capacity(events.len());
    let mut paths_to_extract: Vec<PathBuf> = Vec::with_capacity(events.len());
    for event in events {
        match event {
            FileEvent::Created(path) | FileEvent::Modified(path) => {
                if path.exists() && seen.insert(path.clone()) {
                    paths_to_extract.push(path.clone());
                }
            }
            FileEvent::Renamed { to, .. } => {
                if to.exists() && seen.insert(to.clone()) {
                    paths_to_extract.push(to.clone());
                }
            }
            FileEvent::Removed(_) => {}
            // Caller short-circuits on RescanNeeded before reaching here.
            FileEvent::RescanNeeded => unreachable!(),
        }
    }

    if paths_to_extract.is_empty() {
        return HashMap::new();
    }

    // One blocking task wrapping Rayon file-level parallelism — the same
    // shape as `scan_files_parallel`. A bulk drop into a watched folder can
    // produce thousands of Created events in one batch; fanning out one
    // `spawn_blocking` per file would burst toward tokio's blocking-thread
    // cap and thrash the disk with hundreds of concurrent readers, while
    // Rayon bounds concurrency to the core count.
    let cover_cache = cover_cache.clone();
    let extracted = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;
        paths_to_extract
            .into_par_iter()
            .filter_map(|path| {
                match extract_or_filename_row(&path, &artwork_dir, &cover_cache, false) {
                    Ok(meta) => Some((path, meta)),
                    // Only an unreadable file gets this far; unparseable tags come back
                    // as a filename-derived row rather than a `None`.
                    Err(e) => {
                        log::warn!(
                            "Skipping {}: {}",
                            path.display(),
                            crate::services::describe(&e)
                        );
                        None
                    }
                }
            })
            .collect::<HashMap<_, _>>()
    })
    .await;

    match extracted {
        Ok(results) => results,
        Err(e) => {
            log::warn!("Metadata extraction task panicked: {e}");
            HashMap::new()
        }
    }
}

/// Process a deduplicated batch of file events.
pub(super) async fn process_batch(
    db: &DbPool,
    paths: &Paths,
    cover_cache: &CoverCache,
    events: Vec<FileEvent>,
) -> AppResult<()> {
    let metadata_map = extract_metadata_batch(paths, cover_cache, &events).await;

    let hashes_to_check: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            FileEvent::Created(path) => metadata_map.get(path).map(|m| m.file_hash.clone()),
            _ => None,
        })
        .collect();
    // Hash → lowest-id existing row whose on-disk file has vanished: the
    // move-detection candidates for this batch. Resolved once here (chunked
    // IN query + one stat pass, both before the write tx opens) and handed
    // to `handle_created`, which previously re-issued a per-event
    // `find_track_by_hash` round-trip inside the transaction for the same
    // data. Entries are consumed on a successful re-point so two same-hash
    // Created events can't both steal the one existing row.
    let mut moved_candidates: HashMap<String, (i64, String)> = if hashes_to_check.is_empty() {
        HashMap::new()
    } else {
        // Batch all hash lookups into chunked IN-clause queries.
        // For each hash, we want the lowest-id matching row, so we fetch
        // (file_hash, id, file_path) for the whole set and pick the min id per hash in Rust.
        let rows: Vec<(String, i64, String)> =
            crate::database::chunked_in_query(db.read(), &hashes_to_check, |placeholders| {
                format!(
                    "SELECT file_hash, id, file_path FROM tracks \
                     WHERE file_hash IN ({placeholders})"
                )
            })
            .await
            .unwrap_or_default();

        let mut by_hash: HashMap<String, (i64, String)> = HashMap::new();
        for (hash, id, path) in rows {
            by_hash
                .entry(hash)
                .and_modify(|existing| {
                    if id < existing.0 {
                        *existing = (id, path.clone());
                    }
                })
                .or_insert((id, path));
        }

        // Stat pass off the runtime: keep only candidates whose previous
        // path is gone from disk (genuinely moved, not duplicated).
        tokio::task::spawn_blocking(move || {
            by_hash.retain(|_, (_, path)| !Path::new(path.as_str()).exists());
            by_hash
        })
        .await
        .unwrap_or_default()
    };

    let is_bulk = events.len() > BULK_THRESHOLD;
    let mut tx = db.write().begin().await?;

    if is_bulk {
        queries::stats::disable_stats_triggers(&mut tx).await?;
    }

    // Rows actually inserted / re-pointed / updated / deleted this batch.
    // Gates the post-loop sweeps: a no-op batch (events for untracked
    // files, paths outside library folders) must not pay the full-table
    // album-artwork window-function pass or a stats recalc — mirrors the
    // `any_changes` gate on the scan path (`library/settings/folders.rs`).
    let mut changes: usize = 0;

    for event in &events {
        match event {
            FileEvent::Created(path) => {
                if let Some(meta) = metadata_map.get(path) {
                    match handle_created(&mut tx, path, meta, &mut moved_candidates).await {
                        Ok(changed) => changes += usize::from(changed),
                        Err(e) => {
                            log::warn!("Failed to process created file {}: {}", path.display(), e);
                        }
                    }
                }
            }
            FileEvent::Removed(path) => {
                let path_str = path.to_string_lossy();
                match queries::scan::delete_track_by_path(&mut tx, &path_str).await {
                    Ok(true) => {
                        changes += 1;
                        log::info!("Removed track: {}", path.display());
                    }
                    Ok(false) => log::debug!("Track not in DB, skip remove: {}", path.display()),
                    Err(e) => log::warn!("Failed to remove track {}: {}", path.display(), e),
                }
            }
            FileEvent::Renamed { from, to } => {
                let meta = metadata_map.get(to);
                match handle_renamed(&mut tx, from, to, meta, &mut moved_candidates).await {
                    Ok(changed) => changes += usize::from(changed),
                    Err(e) => log::warn!(
                        "Failed to process rename {} -> {}: {}",
                        from.display(),
                        to.display(),
                        e
                    ),
                }
            }
            FileEvent::Modified(path) => {
                if let Some(meta) = metadata_map.get(path) {
                    match handle_modified(&mut tx, path, meta, &mut moved_candidates).await {
                        Ok(changed) => changes += usize::from(changed),
                        Err(e) => {
                            log::warn!("Failed to process modified file {}: {}", path.display(), e);
                        }
                    }
                }
            }
            // Caller short-circuits on RescanNeeded before reaching here.
            FileEvent::RescanNeeded => unreachable!(),
        }
    }

    if is_bulk {
        // Triggers were off during the loop; with zero changes the
        // denormalized stats are still correct, so only the re-enable is
        // unconditional.
        if changes > 0 {
            queries::stats::recalculate_all_stats(&mut tx).await?;
        }
        queries::stats::enable_stats_triggers(&mut tx).await?;
    }

    if changes > 0 {
        queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
        // A deleted file can empty its album/artist/genre; prune the stranded rows.
        queries::scan::prune_orphans(&mut tx).await?;
    }
    tx.commit().await?;

    Ok(())
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
    moved_candidates: &mut HashMap<String, (i64, String)>,
) -> AppResult<bool> {
    let path_str = path.to_string_lossy().into_owned();

    if queries::scan::track_exists_by_path(tx, &path_str).await? {
        return Ok(false);
    }

    // Move detection: same content hash + the previous owner's path is now
    // missing → re-point the existing row instead of inserting a new one.
    // Candidates were batch-resolved before the transaction opened; consume
    // the entry only on a successful re-point so a failed folder lookup
    // leaves it available to a later same-hash event, matching the old
    // per-event-query behavior.
    if let Some((existing_id, old_path)) = moved_candidates.get(&meta.file_hash).cloned() {
        let Some(folder_id) = queries::scan::find_folder_for_path(tx, &path_str).await? else {
            log::debug!("Moved file not in any library folder, skipping: {}", path.display());
            return Ok(false);
        };
        let file_name = file_name_owned(path);
        // `meta` was extracted from this very path, so its `date_modified` is the
        // mtime already in hand — re-deriving it would `stat` the file again and
        // could pair a fresh mtime with the size/hash of the earlier instant.
        let repointed = queries::scan::update_track_location(
            tx,
            existing_id,
            &path_str,
            &file_name,
            folder_id,
            meta.date_modified.as_deref(),
        )
        .await?;
        if repointed {
            moved_candidates.remove(&meta.file_hash);
            log::info!("Detected moved file: {old_path} -> {path_str}");
            return Ok(true);
        }
        // 0 rows: the candidate row was deleted earlier in this same
        // transaction (dedup emits batch events in arbitrary order, so a
        // Removed for the old path can precede this Created). The pre-tx
        // candidate map can't see in-tx deletes — drop the dead entry and
        // fall through to a fresh insert, matching the old inside-tx
        // `find_track_by_hash` behavior.
        moved_candidates.remove(&meta.file_hash);
    }

    let Some(ids) =
        queries::scan::resolve_track_context(tx, path, &path_str, meta, "Created").await?
    else {
        return Ok(false);
    };

    let file_name = file_name_owned(path);
    let now = crate::utils::now_rfc3339();
    let _new_id = queries::scan::insert_track(tx, &path_str, &file_name, meta, &ids, &now).await?;
    log::info!("Added new track: {path_str}");

    Ok(true)
}

/// Returns `true` when a row was actually written. See [`handle_created`].
async fn handle_renamed(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    from: &Path,
    to: &Path,
    meta: Option<&ExtractedMetadata>,
    moved_candidates: &mut HashMap<String, (i64, String)>,
) -> AppResult<bool> {
    let from_str = from.to_string_lossy().into_owned();
    let to_str = to.to_string_lossy().into_owned();

    if let Some(track_id) = queries::scan::get_track_id_by_path(tx, &from_str).await? {
        let Some(folder_id) = queries::scan::find_folder_for_path(tx, &to_str).await? else {
            log::debug!("Renamed file not in any library folder, skipping: {}", to.display());
            return Ok(false);
        };

        let file_name = file_name_owned(to);
        // Same rule as `handle_created`: `meta` (when present) already carries the
        // mtime `extract_metadata` derived from its own `stat` of `to`. Re-`stat`ing
        // here would store a *newer* mtime beside the size/hash of the previous scan
        // — `update_track_location` doesn't touch those — and an in-place tag edit
        // that happens not to change the size would then read as current forever to
        // `scanner::track_is_current`. An older mtime only ever fails toward a
        // re-parse. The fallback covers the one case with nothing in hand: extraction
        // failed, or `to` vanished before the batch was extracted.
        let date_modified =
            meta.and_then(|m| m.date_modified.clone()).or_else(|| extract_date_modified(to));

        // `track_id` was resolved by path inside this transaction, so the
        // row can't have vanished — the re-point bool is vacuously true.
        let _repointed = queries::scan::update_track_location(
            tx,
            track_id,
            &to_str,
            &file_name,
            folder_id,
            date_modified.as_deref(),
        )
        .await?;
        log::info!("Renamed track: {from_str} -> {to_str}");
        return Ok(true);
    } else if let Some(meta) = meta {
        return handle_created(tx, to, meta, moved_candidates).await;
    }

    Ok(false)
}

/// Returns `true` when a row was actually written. See [`handle_created`].
async fn handle_modified(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    meta: &ExtractedMetadata,
    moved_candidates: &mut HashMap<String, (i64, String)>,
) -> AppResult<bool> {
    let path_str = path.to_string_lossy().into_owned();

    if !queries::scan::track_exists_by_path(tx, &path_str).await? {
        return handle_created(tx, path, meta, moved_candidates).await;
    }

    let Some(ids) =
        queries::scan::resolve_track_context(tx, path, &path_str, meta, "Modified").await?
    else {
        return Ok(false);
    };

    queries::scan::update_track_metadata(tx, &path_str, meta, &ids).await?;
    log::info!("Updated metadata for: {path_str}");

    Ok(true)
}

#[cfg(test)]
#[path = "../tests/reconcile_tests.rs"]
mod tests;
