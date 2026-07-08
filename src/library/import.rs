use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::database::queries;
use crate::entities::track::TrackSummary;
use crate::error::AppError;
use crate::media::AUDIO_EXTENSIONS;
use crate::media::scanner::scan_files_parallel;
use crate::state::AppState;

/// Result of importing files into the library (shared by playlist and queue import).
///
/// `summaries` is populated on demand: callers that need playable
/// `TrackSummary` rows (the queue) ask for them via
/// [`import_files_with_summaries`]; callers that only need ids
/// (playlists) use [`import_files_to_library`] and avoid the extra
/// projection query.
#[derive(Clone, Serialize)]
pub struct ImportFilesResult {
    pub track_ids: Vec<i64>,
    pub imported_count: u32,
    pub failed_paths: Vec<String>,
    #[serde(skip)]
    pub summaries: Vec<Arc<TrackSummary>>,
}

/// Imports audio files into the library, returning track IDs for all valid files
/// (both newly imported and already-existing). Reusable by playlist and queue commands.
pub async fn import_files_to_library(
    state: &AppState,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    let mut failed_paths: Vec<String> = Vec::new();

    let mut valid_paths: Vec<String> = Vec::with_capacity(file_paths.len());
    for file_path in file_paths {
        let path = PathBuf::from(file_path);
        let ext = if let Some(e) = path.extension().and_then(|e| e.to_str()) { e.to_lowercase() } else {
            failed_paths.push(file_path.clone());
            continue;
        };
        if !AUDIO_EXTENSIONS.contains(&ext.as_str()) {
            failed_paths.push(file_path.clone());
            continue;
        }
        if let Ok(canonical) = crate::utils::canonicalize_path(&path) {
            valid_paths.push(canonical.to_string_lossy().into_owned());
        } else {
            failed_paths.push(file_path.clone());
        }
    }

    if valid_paths.is_empty() {
        return Ok(ImportFilesResult {
            track_ids: vec![],
            imported_count: 0,
            failed_paths,
            summaries: Vec::new(),
        });
    }

    let existing_map = queries::track::get_track_ids_by_paths(&state.db, &valid_paths).await?;
    let mut all_track_ids: Vec<i64> = existing_map.values().copied().collect();

    let new_paths: Vec<PathBuf> = valid_paths
        .iter()
        .filter(|p| !existing_map.contains_key(*p))
        .map(PathBuf::from)
        .collect();

    let mut imported_count: u32 = 0;

    if !new_paths.is_empty() {
        let artwork_dir = state.paths.artwork_dir.clone();
        let new_paths_clone = new_paths.clone();
        let cover_cache_clone = state.cover_cache.clone();
        let scanned_files = tokio::task::spawn_blocking(move || {
            scan_files_parallel(&new_paths_clone, &artwork_dir, &cover_cache_clone, &|_, _| {})
        })
        .await
        .map_err(|e| AppError::scanner("Scan task failed", e))?;

        if !scanned_files.is_empty() {
            let mut tx = state.db.write().begin().await?;
            queries::stats::disable_stats_triggers(&mut tx).await?;

            let scan_timestamp = crate::utils::now_rfc3339();

            let result = queries::ingest::ingest_scanned_files(
                &mut tx,
                &scanned_files,
                &queries::FolderResolution::FromParentDir,
                &scan_timestamp,
                false,
            )
            .await?;

            imported_count = result.inserted_count;

            queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
            queries::stats::recalculate_all_stats(&mut tx).await?;
            queries::stats::enable_stats_triggers(&mut tx).await?;
            tx.commit().await?;

            // IDs come from `insert_tracks_batch`'s `RETURNING id, file_path`
            // (remapped to input/drop order via the returned path), collected
            // during ingest — no follow-up `WHERE file_path IN (…)` round-trip.
            all_track_ids.extend(result.inserted_track_ids);
        }
    }

    Ok(ImportFilesResult {
        track_ids: all_track_ids,
        imported_count,
        failed_paths,
        summaries: Vec::new(),
    })
}

/// Like [`import_files_to_library`] but additionally fetches
/// `TrackSummary` rows for every imported id (both newly-inserted and
/// already-existing) in a single `WHERE id IN (...)` query, populated
/// into `ImportFilesResult.summaries`. Use this from the queue's
/// drag-and-drop path so the caller doesn't have to do its own follow-up
/// SELECT.
pub async fn import_files_with_summaries(
    state: &AppState,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    let mut result = import_files_to_library(state, file_paths).await?;
    if !result.track_ids.is_empty() {
        result.summaries = queries::track::get_track_summaries_by_ids(&state.db, &result.track_ids)
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();
    }
    Ok(result)
}
