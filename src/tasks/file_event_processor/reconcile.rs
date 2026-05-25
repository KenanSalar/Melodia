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
use crate::media::metadata::{ExtractedMetadata, extract_date_modified, extract_metadata};
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

    let mut join_set = tokio::task::JoinSet::new();
    for path in paths_to_extract {
        let artwork_dir = artwork_dir.clone();
        let cover_cache_clone = cover_cache.clone();
        join_set.spawn_blocking(move || {
            let result = extract_metadata(&path, &artwork_dir, &cover_cache_clone, false);
            (path, result)
        });
    }

    let mut results = HashMap::new();
    while let Some(task_result) = join_set.join_next().await {
        match task_result {
            Ok((path, Ok(meta))) => {
                results.insert(path, meta);
            }
            Ok((path, Err(e))) => {
                log::warn!("Failed to extract metadata for {}: {}", path.display(), e);
            }
            Err(e) => {
                log::warn!("Metadata extraction task panicked: {e}");
            }
        }
    }

    results
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
    let missing_old_paths: HashSet<String> = if hashes_to_check.is_empty() {
        HashSet::new()
    } else {
        // Batch all hash lookups into chunked IN-clause queries.
        // For each hash, we want the lowest-id matching row, so we fetch
        // (file_hash, id, file_path) for the whole set and pick the min id per hash in Rust.
        let rows: Vec<(String, i64, String)> = crate::database::chunked_in_query(
            db.read(),
            &hashes_to_check,
            |placeholders| {
                format!(
                    "SELECT file_hash, id, file_path FROM tracks \
                     WHERE file_hash IN ({placeholders})"
                )
            },
        )
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
        let candidates: Vec<String> = by_hash.into_values().map(|(_, p)| p).collect();

        tokio::task::spawn_blocking(move || {
            candidates
                .into_iter()
                .filter(|p| !Path::new(p).exists())
                .collect::<HashSet<String>>()
        })
        .await
        .unwrap_or_default()
    };

    let is_bulk = events.len() > BULK_THRESHOLD;
    let mut tx = db.write().begin().await?;

    if is_bulk {
        queries::stats::disable_stats_triggers(&mut tx).await?;
    }

    for event in &events {
        match event {
            FileEvent::Created(path) => {
                if let Some(meta) = metadata_map.get(path)
                    && let Err(e) = handle_created(&mut tx, path, meta, &missing_old_paths).await
                {
                    log::warn!("Failed to process created file {}: {}", path.display(), e);
                }
            }
            FileEvent::Removed(path) => {
                let path_str = path.to_string_lossy();
                match queries::scan::delete_track_by_path(&mut tx, &path_str).await {
                    Ok(true) => log::info!("Removed track: {}", path.display()),
                    Ok(false) => log::debug!("Track not in DB, skip remove: {}", path.display()),
                    Err(e) => log::warn!("Failed to remove track {}: {}", path.display(), e),
                }
            }
            FileEvent::Renamed { from, to } => {
                let meta = metadata_map.get(to);
                if let Err(e) = handle_renamed(&mut tx, from, to, meta, &missing_old_paths).await {
                    log::warn!(
                        "Failed to process rename {} -> {}: {}",
                        from.display(),
                        to.display(),
                        e
                    );
                }
            }
            FileEvent::Modified(path) => {
                if let Some(meta) = metadata_map.get(path)
                    && let Err(e) = handle_modified(&mut tx, path, meta, &missing_old_paths).await
                {
                    log::warn!("Failed to process modified file {}: {}", path.display(), e);
                }
            }
            // Caller short-circuits on RescanNeeded before reaching here.
            FileEvent::RescanNeeded => unreachable!(),
        }
    }

    if is_bulk {
        queries::stats::recalculate_all_stats(&mut tx).await?;
        queries::stats::enable_stats_triggers(&mut tx).await?;
    }

    queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
    tx.commit().await?;

    Ok(())
}

/// Resolve a path's library-folder + artist/album/genre rows, upserting any
/// missing rows. Returns `None` (with a debug log under `context`) when the
/// path is not inside any library folder — callers should short-circuit.
async fn resolve_track_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    path_str: &str,
    meta: &ExtractedMetadata,
    context: &str,
) -> AppResult<Option<queries::ResolvedIds>> {
    let Some(folder_id) = queries::scan::find_folder_for_path(tx, path_str).await? else {
        log::debug!(
            "{context} file not in any library folder, skipping: {}",
            path.display()
        );
        return Ok(None);
    };

    let artist_name = meta.artist.as_deref().unwrap_or("");
    let album_name = meta.album.as_deref().unwrap_or("");
    let genre_name = meta.genre.as_deref().unwrap_or("");

    let artist_id = queries::scan::upsert_artist(tx, artist_name, 1).await?;
    let album_id = queries::scan::upsert_album(tx, album_name, artist_id, meta.year).await?;
    let genre_id = queries::scan::upsert_genre(tx, genre_name).await?;

    Ok(Some(queries::ResolvedIds {
        artist_id,
        album_id,
        genre_id,
        folder_id,
    }))
}

fn file_name_owned(path: &Path) -> String {
    path.file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_owned()
}

async fn handle_created(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    meta: &ExtractedMetadata,
    missing_old_paths: &HashSet<String>,
) -> AppResult<()> {
    let path_str = path.to_string_lossy().into_owned();

    if queries::scan::track_exists_by_path(tx, &path_str).await? {
        return Ok(());
    }

    // Move detection: same content hash + the previous owner's path is now
    // missing → re-point the existing row instead of inserting a new one.
    if let Some((existing_id, old_path)) = queries::scan::find_track_by_hash(tx, &meta.file_hash).await?
        && missing_old_paths.contains(&old_path)
    {
        let Some(folder_id) = queries::scan::find_folder_for_path(tx, &path_str).await? else {
            log::debug!(
                "Moved file not in any library folder, skipping: {}",
                path.display()
            );
            return Ok(());
        };
        let file_name = file_name_owned(path);
        let date_modified = extract_date_modified(path);
        queries::scan::update_track_location(
            tx,
            existing_id,
            &path_str,
            &file_name,
            folder_id,
            date_modified.as_deref(),
        )
        .await?;
        log::info!("Detected moved file: {old_path} -> {path_str}");
        return Ok(());
    }

    let Some(ids) = resolve_track_context(tx, path, &path_str, meta, "Created").await? else {
        return Ok(());
    };

    let file_name = file_name_owned(path);
    let now = crate::utils::now_rfc3339();
    let _new_id =
        queries::scan::insert_track(tx, &path_str, &file_name, meta, &ids, &now).await?;
    log::info!("Added new track: {path_str}");

    Ok(())
}

async fn handle_renamed(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    from: &Path,
    to: &Path,
    meta: Option<&ExtractedMetadata>,
    missing_old_paths: &HashSet<String>,
) -> AppResult<()> {
    let from_str = from.to_string_lossy().into_owned();
    let to_str = to.to_string_lossy().into_owned();

    if let Some(track_id) = queries::scan::get_track_id_by_path(tx, &from_str).await? {
        let Some(folder_id) = queries::scan::find_folder_for_path(tx, &to_str).await? else {
            log::debug!(
                "Renamed file not in any library folder, skipping: {}",
                to.display()
            );
            return Ok(());
        };

        let file_name = file_name_owned(to);
        let date_modified = extract_date_modified(to);

        queries::scan::update_track_location(
            tx,
            track_id,
            &to_str,
            &file_name,
            folder_id,
            date_modified.as_deref(),
        )
        .await?;
        log::info!("Renamed track: {from_str} -> {to_str}");
    } else if let Some(meta) = meta {
        handle_created(tx, to, meta, missing_old_paths).await?;
    }

    Ok(())
}

async fn handle_modified(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    meta: &ExtractedMetadata,
    missing_old_paths: &HashSet<String>,
) -> AppResult<()> {
    let path_str = path.to_string_lossy().into_owned();

    if !queries::scan::track_exists_by_path(tx, &path_str).await? {
        return handle_created(tx, path, meta, missing_old_paths).await;
    }

    let Some(ids) = resolve_track_context(tx, path, &path_str, meta, "Modified").await? else {
        return Ok(());
    };

    queries::scan::update_track_metadata(tx, &path_str, meta, &ids).await?;
    log::info!("Updated metadata for: {path_str}");

    Ok(())
}
