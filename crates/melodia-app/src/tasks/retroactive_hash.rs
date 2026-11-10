//! Hashes every track whose `file_hash` is still NULL, at each startup and after a folder add; a
//! no-op once the column is full.

use std::collections::HashMap;
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
    let unhashed = by_file(queries::track::get_unhashed_track_paths(db).await?);
    if unhashed.is_empty() {
        return Ok(());
    }

    log::info!("Starting retroactive hashing for {} files", unhashed.len());

    let hashed = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;

        ScanPool::for_files(unhashed.len()).install(|| {
            unhashed
                .into_par_iter()
                .filter_map(|(path, ids)| hash_file(&path).map(|(hash, mtime)| (ids, hash, mtime)))
                .collect::<Vec<_>>()
        })
    })
    .await
    .map_err(|e| melodia_core::error::AppError::scanner("Hashing task panicked", e))?;

    if hashed.is_empty() {
        return Ok(());
    }

    log::info!("Retroactive hashing complete: {} files hashed, writing to database", hashed.len());

    let updates = hashed
        .into_iter()
        .flat_map(|(ids, hash, mtime)| {
            ids.into_iter().map(move |id| (id, hash.clone(), mtime.clone()))
        })
        .collect();
    queries::track::batch_update_hashes(db, updates).await?;

    log::info!("Retroactive hash database update complete");
    Ok(())
}

/// Each file once, with the ids of its rows: a cue sheet's tracks share their image, which can run
/// to a whole disc.
fn by_file(rows: Vec<(i64, String)>) -> Vec<(String, Vec<i64>)> {
    let mut files: HashMap<String, Vec<i64>> = HashMap::new();
    for (id, path) in rows {
        files.entry(path).or_default().push(id);
    }
    files.into_iter().collect()
}

/// The file's hash and mtime, or `None` where it is gone or unreadable.
fn hash_file(path_str: &str) -> Option<(String, Option<String>)> {
    let path = Path::new(path_str);
    // One `stat` proves the file is there and dates it from the same instant.
    let Ok(meta) = std::fs::metadata(path) else {
        log::debug!("Skipping missing file during retroactive hash: {path_str}");
        return None;
    };

    let hash = match melodia_store::media::ingest::metadata::compute_file_hash(path) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("Failed to hash {path_str}: {}", describe(&e));
            return None;
        }
    };

    let mtime = melodia_store::media::ingest::metadata::date_modified_from_metadata(&meta);
    Some((hash, mtime))
}

#[cfg(test)]
#[path = "tests/retroactive_hash_tests.rs"]
mod tests;
