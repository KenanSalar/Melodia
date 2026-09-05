use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_core::utils::audio_ext::is_audio_extension;
use melodia_store::database::{DbPool, queries};
use melodia_store::media::ingest::scanner::scan_files_parallel;

/// Result of importing files into the library (shared by playlist and queue import).
///
/// `summaries` costs a second projection query, so [`import_files`] leaves it empty and
/// [`import_and_summarize`] is the variant that fills it.
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
///
/// Takes the three pieces it reaches rather than an `&AppState`, as
/// [`super::tags::write_tag_edit`] does, so a drop can be replayed against a `test_pool`.
async fn import_files(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    let mut failed_paths: Vec<String> = Vec::new();

    let mut valid_paths: Vec<String> = Vec::with_capacity(file_paths.len());
    for file_path in file_paths {
        let path = PathBuf::from(file_path);
        let is_audio = path.extension().and_then(|e| e.to_str()).is_some_and(is_audio_extension);
        if !is_audio {
            failed_paths.push(file_path.clone());
            continue;
        }
        if let Ok(canonical) = melodia_core::utils::canonicalize_path(&path) {
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

    let existing_map = queries::track::get_track_ids_by_paths(db, &valid_paths).await?;
    let mut all_track_ids: Vec<i64> = existing_map.values().copied().collect();

    let new_paths: Vec<PathBuf> =
        valid_paths.iter().filter(|p| !existing_map.contains_key(*p)).map(PathBuf::from).collect();

    let mut imported_count: u32 = 0;

    if !new_paths.is_empty() {
        let artwork_dir = artwork_dir.to_path_buf();
        let new_paths_clone = new_paths.clone();
        let cover_cache_clone = cover_cache.clone();
        let scanned_files = tokio::task::spawn_blocking(move || {
            scan_files_parallel(&new_paths_clone, &artwork_dir, &cover_cache_clone, &|_, _| {})
        })
        .await
        .map_err(|e| AppError::scanner("Scan task failed", e))?;

        if !scanned_files.is_empty() {
            let mut tx = db.write().begin().await?;
            queries::stats::disable_stats_triggers(&mut tx).await?;

            let scan_timestamp = melodia_core::utils::now_rfc3339();

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

/// The library door onto a drop, for a caller holding an `&AppState`: it is the one place the
/// three fields below are spelled out of one, rather than at each queue entry point.
pub(crate) async fn import_files_with_summaries(
    state: &AppState,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    import_and_summarize(&state.db, &state.paths.artwork_dir, &state.cover_cache, file_paths).await
}

/// Like [`import_files`] but additionally fetches `TrackSummary` rows for every imported id
/// (both newly-inserted and already-existing) in a single `WHERE id IN (...)` query, populated
/// into `ImportFilesResult.summaries`, so a drop path doesn't have to do its own follow-up
/// SELECT.
pub(crate) async fn import_and_summarize(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    let mut result = import_files(db, artwork_dir, cover_cache, file_paths).await?;
    if !result.track_ids.is_empty() {
        result.summaries = queries::track::get_track_summaries_by_ids(db, &result.track_ids)
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();
    }
    Ok(result)
}

#[cfg(test)]
#[path = "tests/import_tests.rs"]
mod tests;
