//! Auto-tag backfill writer: stamp a resolved `MusicBrainz` Recording ID into a
//! batch of files and land the byte-consistent DB rows in one transaction.
//!
//! This is the per-file-MBID sibling of [`crate::library::tags`]. The tag editor
//! applies **one** [`TagEdit`] to many tracks; the backfill writes a *different*
//! id per file, so it can't share that path's single-edit machinery — but it
//! reuses the same primitives: mark the write in [`SelfWrites`] first so the
//! watcher ignores the echo, rewrite the tag, re-extract (recomputing
//! `file_hash` / `file_size` / `date_modified` so `scanner::track_is_current`
//! stays honest), then `update_track_metadata` inside one write transaction.
//!
//! It deliberately does **not** bump `library_changed_tx`: the Recording ID is
//! never displayed, so no view is stale, and staying silent keeps the backfill
//! task — which subscribes to that channel — from waking itself in a loop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;

use crate::database::{DbPool, queries};
use crate::error::AppError;
use crate::media::artwork::CoverCache;
use crate::media::metadata::{ExtractedMetadata, extract_metadata};
use crate::media::self_writes::SelfWrites;
use crate::media::tag_writer::{self, FieldEdit, TagEdit};
use crate::services::describe;
use crate::state::AppState;

/// Same bound as the tag editor's fan-out ([`crate::library::tags`]): the write
/// is I/O-bound and the MP4 save clones the embedded cover, so extra width buys
/// nothing and costs memory.
const MBID_WRITE_THREADS: usize = 4;

/// One recording id resolved for a track: `(track id, DB file path, recording
/// mbid)`. The path is the DB's stored `file_path` verbatim so the re-extract and
/// the `SelfWrites` mark key on the same string the watcher sees.
pub type ResolvedMbid = (i64, String, String);

/// Write each resolved Recording ID into its file and refresh the DB rows in one
/// transaction. Returns the number of rows successfully updated. Per-file write
/// or re-extract failures are logged and skipped, never fatal.
pub async fn write_resolved_mbids(
    state: &AppState,
    resolved: &[ResolvedMbid],
) -> Result<usize, AppError> {
    if resolved.is_empty() {
        return Ok(0);
    }
    write_mbids(
        &state.db,
        &state.paths.artwork_dir,
        &state.cover_cache,
        &state.self_writes,
        resolved,
    )
    .await
}

/// Testable core: takes only the pieces it needs (no `AppState`) so it runs
/// against a `test_pool` + real temp files.
pub(crate) async fn write_mbids(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &Arc<SelfWrites>,
    resolved: &[ResolvedMbid],
) -> Result<usize, AppError> {
    let files = {
        let resolved = resolved.to_vec();
        let artwork_dir = artwork_dir.to_path_buf();
        let cover_cache = cover_cache.clone();
        let self_writes = Arc::clone(self_writes);
        tokio::task::spawn_blocking(move || {
            run_write_pass(&resolved, &artwork_dir, &cover_cache, &self_writes)
        })
        .await
        .map_err(|e| AppError::metadata_msg(format!("mbid write task panicked: {e}")))?
    };

    match run_commit(db, &files).await {
        Ok(count) => Ok(count),
        Err(e) => {
            // Only on the (rare) tx failure: unmark so the watcher re-ingests
            // instead of leaving a fresh mtime beside the pre-write DB row.
            let marked: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(&f.path)).collect();
            self_writes.unmark(&marked);
            Err(e)
        }
    }
}

/// Per-file result carried out of the blocking pass.
struct FileWrite {
    path: String,
    outcome: Result<ExtractedMetadata, AppError>,
}

/// Rewrite each file's Recording ID on a bounded Rayon pool, then re-extract.
fn run_write_pass(
    resolved: &[ResolvedMbid],
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &SelfWrites,
) -> Vec<FileWrite> {
    let map_files = || {
        resolved
            .par_iter()
            .map(|(_id, path, mbid)| {
                let edit = TagEdit {
                    musicbrainz_track_id: FieldEdit::Set(mbid.clone()),
                    ..TagEdit::default()
                };
                let p = Path::new(path);
                // Mark BEFORE the write (with the DB `file_path`, on which the set
                // keys by exact equality) so the watcher drops the echo. Skip
                // artwork on re-extract — the MBID write leaves the cover untouched
                // and `update_track_metadata` COALESCEs a NULL artwork to the
                // existing value.
                self_writes.mark(p);
                let outcome = tag_writer::apply_to_file(p, &edit, None)
                    .and_then(|_| extract_metadata(p, artwork_dir, cover_cache, true));
                FileWrite {
                    path: path.clone(),
                    outcome,
                }
            })
            .collect::<Vec<FileWrite>>()
    };

    match rayon::ThreadPoolBuilder::new().num_threads(MBID_WRITE_THREADS).build() {
        Ok(pool) => pool.install(map_files),
        Err(e) => {
            log::warn!("mbid-write pool build failed ({e}); using the global pool");
            map_files()
        }
    }
}

/// FK-resolution memo key — identical to the tag editor's, so tracks sharing
/// folder + artist + album/year + genre resolve their ids once per batch.
type ResolveKey = (PathBuf, String, String, Option<i32>, String);

/// Land the successful writes in one transaction: resolve ids and refresh each
/// track row. Returns the number of rows updated.
async fn run_commit(db: &DbPool, files: &[FileWrite]) -> Result<usize, AppError> {
    let mut tx = db.write().begin().await?;
    let mut resolve_cache: HashMap<ResolveKey, queries::scan::ResolvedIds> = HashMap::new();
    let mut updated = 0usize;

    for f in files {
        let meta = match &f.outcome {
            Ok(meta) => meta,
            Err(e) => {
                log::warn!("mbid write failed for {}: {}", f.path, describe(e));
                continue;
            }
        };
        let path = Path::new(&f.path);
        let key: ResolveKey = (
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
            meta.artist.clone().unwrap_or_default(),
            meta.album.clone().unwrap_or_default(),
            meta.year,
            meta.genre.clone().unwrap_or_default(),
        );
        let rids = if let Some(cached) = resolve_cache.get(&key) {
            *cached
        } else {
            let Some(resolved) =
                queries::scan::resolve_track_context(&mut tx, path, &f.path, meta, "MBID backfill")
                    .await?
            else {
                log::warn!("mbid backfill: {} not in a library folder", f.path);
                continue;
            };
            resolve_cache.insert(key, resolved);
            resolved
        };
        queries::scan::update_track_metadata(&mut tx, &f.path, meta, &rids).await?;
        updated += 1;
    }

    tx.commit().await?;
    Ok(updated)
}

#[cfg(test)]
#[path = "tests/mbid_tests.rs"]
mod tests;
