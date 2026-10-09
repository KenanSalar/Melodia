//! Apply a deduplicated batch of file events to the database: extract
//! metadata for created/modified/renamed paths, then hand the batch to
//! [`queries::scan::apply_watch_batch`], which writes the track rows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::config::Paths;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::{AppResult, describe};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::database::queries::scan::WatchedChange;
use melodia_store::media::ingest::metadata::{extract_date_modified, extract_or_filename_row};
use melodia_store::media::ingest::scan_pool::ScanPool;
use melodia_store::media::ingest::watcher::FileEvent;

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
        ScanPool::for_files(paths_to_extract.len()).install(|| {
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
                                melodia_core::error::describe(&e)
                            );
                            None
                        }
                    }
                })
                .collect::<HashMap<_, _>>()
        })
    })
    .await;

    match extracted {
        Ok(results) => results,
        Err(e) => {
            log::warn!("Metadata extraction task panicked: {}", describe(&e));
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
    let metadata = extract_metadata_batch(paths, cover_cache, &events).await;
    queries::scan::apply_watch_batch(db, watched_changes(events, metadata)).await
}

/// Pairs each event with what extraction read, dropping a create or a modify whose file could not
/// be read. Dedup leaves one event per path, so each entry is moved out of `metadata` rather than
/// cloned.
fn watched_changes(
    events: Vec<FileEvent>,
    mut metadata: HashMap<PathBuf, ExtractedMetadata>,
) -> Vec<WatchedChange> {
    events
        .into_iter()
        .filter_map(|event| match event {
            FileEvent::Created(path) => {
                let meta = metadata.remove(&path)?;
                Some(WatchedChange::Created { path, meta })
            }
            FileEvent::Modified(path) => {
                let meta = metadata.remove(&path)?;
                Some(WatchedChange::Modified { path, meta })
            }
            FileEvent::Renamed { from, to } => {
                let meta = metadata.remove(&to);
                // `WatchedChange::Renamed` argues why `meta`'s mtime wins. The `stat` is for the
                // one case with nothing in hand: extraction failed, or `to` vanished first.
                let date_modified = meta
                    .as_ref()
                    .and_then(|meta| meta.date_modified.clone())
                    .or_else(|| extract_date_modified(&to));
                Some(WatchedChange::Renamed { from, to, meta, date_modified })
            }
            FileEvent::Removed(path) => Some(WatchedChange::Removed(path)),
            // Caller short-circuits on RescanNeeded before reaching here.
            FileEvent::RescanNeeded => unreachable!(),
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/reconcile_tests.rs"]
mod tests;
