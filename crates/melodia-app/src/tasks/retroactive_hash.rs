//! Hashes every track whose `file_hash` is still NULL, at each startup and after a folder add; a
//! no-op once the column is full.

use std::path::Path;

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::{AppResult, describe};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::media::ingest::scan_pool::ScanPool;

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let db = state.db.clone();
    spawner.spawn(async move {
        if let Err(e) = hash_unhashed_tracks(&db).await {
            log::warn!("Background hashing failed: {}", describe(&e));
        }
    });
    log::info!("Retroactive hash task started");
}

async fn hash_unhashed_tracks(db: &DbPool) -> AppResult<()> {
    let unhashed = queries::track::get_unhashed_track_paths(db).await?;
    if unhashed.is_empty() {
        return Ok(());
    }

    log::info!("Starting retroactive hashing for {} tracks", unhashed.len());

    let updates = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;

        ScanPool::for_files(unhashed.len()).install(|| {
            unhashed
                .par_iter()
                .filter_map(|(id, path_str)| {
                    let path = Path::new(path_str);
                    // One `stat` proves the file is there and dates it from the same instant.
                    let Ok(meta) = std::fs::metadata(path) else {
                        log::debug!("Skipping missing file during retroactive hash: {path_str}");
                        return None;
                    };

                    let hash = match melodia_store::media::ingest::metadata::compute_file_hash(path)
                    {
                        Ok(h) => h,
                        Err(e) => {
                            log::warn!("Failed to hash {path_str}: {}", describe(&e));
                            return None;
                        }
                    };

                    let mtime =
                        melodia_store::media::ingest::metadata::date_modified_from_metadata(&meta);

                    Some((*id, hash, mtime))
                })
                .collect::<Vec<_>>()
        })
    })
    .await
    .map_err(|e| melodia_core::error::AppError::scanner("Hashing task panicked", e))?;

    if updates.is_empty() {
        return Ok(());
    }

    log::info!("Retroactive hashing complete: {} files hashed, writing to database", updates.len());

    queries::track::batch_update_hashes(db, &updates).await?;

    log::info!("Retroactive hash database update complete");
    Ok(())
}

#[cfg(test)]
#[path = "tests/retroactive_hash_tests.rs"]
mod tests;
