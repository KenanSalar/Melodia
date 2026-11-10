//! Applies a deduplicated batch: settles the cue sheets it touches, extracts metadata for created,
//! modified and renamed paths and the images the sheets read, then hands the batch to
//! [`queries::scan::apply_watch_batch`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::config::Paths;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::{AppResult, describe};
use melodia_store::database::DbPool;
use melodia_store::database::queries;
use melodia_store::database::queries::scan::WatchedChange;
use melodia_store::media::ingest::metadata::{extract_date_modified, extract_or_filename_row};
use melodia_store::media::ingest::scan_pool::ScanPool;
use melodia_store::media::ingest::watcher::{FileEvent, is_sheet_file};

use super::sheets::{self, SheetBatch};

/// The paths whose tags the batch needs, each once, a cue sheet having none. A kept-out file written
/// to is read too, for its exclusion to take the new hash; one renamed away needs no read, its
/// exclusion following the name.
fn paths_to_extract(events: &[FileEvent], kept_out_renamed: &HashSet<String>) -> Vec<PathBuf> {
    let is_kept_out = |path: &Path| kept_out_renamed.contains(path.to_string_lossy().as_ref());
    let mut seen = HashSet::with_capacity(events.len());
    let mut paths: Vec<PathBuf> = Vec::with_capacity(events.len());
    for event in events {
        let wanted = match event {
            FileEvent::Created(path) | FileEvent::Modified(path) => Some(path),
            FileEvent::Renamed { from, to } => (!is_kept_out(from)).then_some(to),
            FileEvent::Removed(_) => None,
            // The caller reconciles on these, or has dropped a gone path holding no tracks.
            FileEvent::RescanNeeded | FileEvent::DirectoryRenamed | FileEvent::DirectoryGone(_) => {
                unreachable!()
            }
        };
        if let Some(path) = wanted
            && !is_sheet_file(path)
            && path.exists()
            && seen.insert(path.clone())
        {
            paths.push(path.clone());
        }
    }
    paths
}

/// The kept-out paths the batch renames away, so the parse of their new names can be skipped. A
/// failed read only costs that saving: the write transaction asks again before it inserts anything.
async fn kept_out_renamed(db: &DbPool, events: &[FileEvent]) -> HashSet<String> {
    let paths: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            FileEvent::Renamed { from, .. } => Some(from.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    queries::excluded_files::among(db, paths).await.unwrap_or_else(|e| {
        log::warn!("Kept-out check skipped for this batch: {}", describe(&e));
        HashSet::new()
    })
}

/// Outside any transaction, so the write lock isn't held across the I/O.
async fn extract_metadata_batch(
    paths: &Paths,
    cover_cache: &CoverCache,
    paths_to_extract: Vec<PathBuf>,
) -> HashMap<PathBuf, ExtractedMetadata> {
    let artwork_dir = paths.artwork_dir.clone();

    if paths_to_extract.is_empty() {
        return HashMap::new();
    }

    // One blocking task over Rayon, `scan_files_parallel`'s shape: a bulk drop can bring thousands
    // of events, and a `spawn_blocking` each would flood the blocking pool and the disk.
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

pub(super) async fn process_batch(
    db: &DbPool,
    paths: &Paths,
    cover_cache: &CoverCache,
    events: Vec<FileEvent>,
) -> AppResult<()> {
    let kept_out_renamed = kept_out_renamed(db, &events).await;
    let SheetBatch { events, cue } = sheets::settle(db, events).await;
    let mut to_extract = paths_to_extract(&events, &kept_out_renamed);
    to_extract.extend(cue.images());
    to_extract.sort_unstable();
    to_extract.dedup();
    let mut metadata = extract_metadata_batch(paths, cover_cache, to_extract).await;
    // First: a file renamed onto a name the plan settled gives its read to the sheet's job, which
    // takes over the rows the rename re-points.
    let mut changes: Vec<WatchedChange> = sheets::change(cue, &mut metadata).into_iter().collect();
    changes.extend(watched_changes(events, metadata));
    queries::scan::apply_watch_batch(db, changes).await
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
            FileEvent::Removed(path) => {
                let gone = !path.exists();
                Some(WatchedChange::Removed { path, gone })
            }
            // The caller reconciles on these, or has dropped a gone path holding no tracks.
            FileEvent::RescanNeeded | FileEvent::DirectoryRenamed | FileEvent::DirectoryGone(_) => {
                unreachable!()
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/reconcile_tests.rs"]
mod tests;
