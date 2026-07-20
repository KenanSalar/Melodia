//! Edit Track Information orchestrator: turn a [`TagEdit`] plus a set of track
//! ids into rewritten files, a byte-consistent DB, and a refreshed player.
//!
//! Everything *after* the tag write reuses the scan pipeline: re-extract the
//! file ([`extract_metadata`]), resolve the artist/album/genre ids
//! ([`queries::scan::resolve_track_context`]) and call
//! [`queries::scan::update_track_metadata`] — which recomputes `file_hash` /
//! `file_size` / `date_modified` / `sort_key` / `duration_ms` and lets the
//! existing FTS / stats triggers do the reindex and rollups with no new SQL.
//!
//! Do **not** shortcut this into a hand-built `UPDATE` from the form values: a
//! tag write rewrites the file's bytes, so hash/size/mtime all change, and a
//! fresh `date_modified` beside a stale `file_hash` is the one state
//! `scanner::track_is_current` reads as current forever. The re-extract is what
//! keeps those three honest.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;

use crate::database::{DbPool, queries};
use crate::entities::track::TrackSummary;
use crate::error::AppError;
use crate::media::artwork::{self, CoverCache};
use crate::media::metadata::{ExtractedMetadata, extract_metadata};
use crate::media::self_writes::SelfWrites;
use crate::media::tag_writer::{self, ArtworkEdit, TagEdit};
use crate::state::AppState;

/// Width cap for the tag-write fan-out. The MP4 save clones the embedded cover,
/// so an unbounded `par_iter` would hold `num_cpus × (image + its clone)`
/// resident at once — a memory regression in a project that exists because of
/// them. Tag writing is I/O-bound, so extra width buys nothing anyway.
const TAG_WRITE_THREADS: usize = 4;

/// Outcome of applying a [`TagEdit`] to a batch of tracks. Partial success is
/// reported, never rolled back — a read-only file or an unsupported container
/// fails only its own row.
#[derive(Debug, Default)]
pub struct TagEditReport {
    pub updated: usize,
    /// `(file, error)` for files whose write or re-extract failed.
    pub failures: Vec<(String, String)>,
    /// `(file, fields)` the file's tag format has no key for — a rare safety
    /// net, since all three primary tag types map every exposed field.
    pub unsupported: Vec<(String, Vec<&'static str>)>,
}

/// Apply `edit` to `ids`, then refresh the player's cached summaries and bump
/// `library_changed_tx` so the visibility-gated list views re-fetch.
///
/// `artwork_source` is `Some(path)` iff `edit.artwork == ArtworkEdit::Replace`
/// (the picked image file): the writer's `Replace` is a unit variant, so the
/// path rides alongside the edit and the orchestrator owns it (it needs it for
/// [`artwork::cache_image_file`] anyway).
pub async fn apply_tag_edit(
    state: &AppState,
    ids: Vec<i64>,
    edit: TagEdit,
    artwork_source: Option<PathBuf>,
) -> Result<TagEditReport, AppError> {
    // lofty rewrites the tag whether or not anything differs, so a reflexive
    // open-then-Save must not touch disk.
    if ids.is_empty() || edit.is_noop() {
        return Ok(TagEditReport::default());
    }

    let (report, updated_ids) = write_tag_edit(
        &state.db,
        &state.paths.artwork_dir,
        &state.cover_cache,
        &state.self_writes,
        &ids,
        &edit,
        artwork_source.as_deref(),
    )
    .await?;

    if !updated_ids.is_empty() {
        // Overwrite any queued / currently-playing summary with its fresh copy
        // so the Now-Playing bar, Queue Sheet and Up Next stop showing old tags.
        let fresh = queries::track::get_track_summaries_by_ids(&state.db, &updated_ids).await?;
        let map: HashMap<i64, TrackSummary> = fresh.into_iter().map(|t| (t.id, t)).collect();
        crate::player::state::sync_track_summaries(&state.player_state, &state.sinks, &map);

        state
            .library_changed_tx
            .send_modify(|n| *n = n.wrapping_add(1));
    }

    Ok(report)
}

/// The testable core: rewrite each file's tags, re-extract, and land the batch
/// in one write transaction. Takes only the pieces it needs (no `AppState`, no
/// player, no watch channel) so it can be exercised with a `test_pool` and real
/// temp files. Returns the report plus the ids whose rows were refreshed (for
/// the caller's player resync).
pub(crate) async fn write_tag_edit(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &Arc<SelfWrites>,
    ids: &[i64],
    edit: &TagEdit,
    artwork_source: Option<&Path>,
) -> Result<(TagEditReport, Vec<i64>), AppError> {
    let mut report = TagEditReport::default();

    let rows = queries::track::get_track_paths_by_ids(db, ids).await?;
    if rows.is_empty() {
        return Ok((report, Vec::new()));
    }

    // Owned copy so the picked-cover decode can move into the blocking task.
    let artwork_source = artwork_source.map(Path::to_path_buf);

    // Blocking, width-capped file write + re-extract pass. The picked cover is
    // decoded + cached *inside* the task too: `cover_picture_from_path` (image
    // decode) and `cache_image_file` (file read + copy) are blocking I/O + CPU,
    // so they belong off the async worker. The cover is handled FIRST — a corrupt
    // pick fails the whole edit (via `?`) before any file is touched.
    let (files, cached_artwork): (Vec<FileWrite>, Option<String>) = {
        let edit = edit.clone();
        let artwork_dir = artwork_dir.to_path_buf();
        let cover_cache = cover_cache.clone();
        let self_writes = Arc::clone(self_writes);
        tokio::task::spawn_blocking(move || {
            // `cover_picture_from_path` decode-validates (lofty only sniffs 8
            // bytes) and re-encodes non-JPEG/PNG; `cache_image_file` copies the
            // original bytes into the content-addressed artwork dir for the DB
            // `artwork_path` write.
            let (picture, cached_artwork) = if edit.artwork == ArtworkEdit::Replace {
                let source = artwork_source.as_deref().ok_or_else(|| {
                    AppError::metadata_msg(
                        "artwork Replace requested without a source image".to_owned(),
                    )
                })?;
                let picture = tag_writer::cover_picture_from_path(source)?;
                let cached = artwork::cache_image_file(source, &artwork_dir);
                (Some(picture), cached)
            } else {
                (None, None)
            };

            let files = run_write_pass(
                &rows,
                &edit,
                picture.as_ref(),
                &artwork_dir,
                &cover_cache,
                &self_writes,
            );
            Ok::<_, AppError>((files, cached_artwork))
        })
        .await
        .map_err(|e| AppError::metadata_msg(format!("tag write task panicked: {e}")))??
    };

    // Every path we marked before writing — unmark on a tx failure so the
    // watcher re-ingests instead of leaving the DB permanently stale.
    let marked: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(&f.path)).collect();

    let updated_ids = match run_commit(db, &files, edit, cached_artwork.as_deref(), &mut report).await
    {
        Ok(ids) => ids,
        Err(e) => {
            self_writes.unmark(&marked);
            return Err(e);
        }
    };

    report.updated = updated_ids.len();
    Ok((report, updated_ids))
}

/// Per-file result carried out of the blocking pass. `Ok((meta, unsupported))`
/// on a successful write + re-extract; `Err(msg)` for a write or re-extract
/// failure (collected, never fatal).
struct FileWrite {
    id: i64,
    path: String,
    outcome: Result<(ExtractedMetadata, Vec<&'static str>), String>,
}

/// Rewrite each file's tags on a bounded Rayon pool, then re-extract. Mirrors
/// `scanner::scan_files_parallel`, but the pool is capped (see
/// [`TAG_WRITE_THREADS`]).
fn run_write_pass(
    rows: &[(i64, String)],
    edit: &TagEdit,
    picture: Option<&lofty::picture::Picture>,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    self_writes: &SelfWrites,
) -> Vec<FileWrite> {
    let map_files = || {
        rows.par_iter()
            .map(|(id, path)| {
                let p = Path::new(path);
                // Mark BEFORE the write, per file, so the TTL clock tracks the
                // event this write is about to fire — and with the DB `file_path`
                // (the set keys on exact `PathBuf` equality).
                self_writes.mark(p);
                let outcome = match tag_writer::apply_to_file(p, edit, picture) {
                    Ok(unsupported) => match extract_metadata(p, artwork_dir, cover_cache, false) {
                        Ok(meta) => Ok((meta, unsupported.0)),
                        Err(e) => Err(e.to_string()),
                    },
                    Err(e) => Err(e.to_string()),
                };
                FileWrite {
                    id: *id,
                    path: path.clone(),
                    outcome,
                }
            })
            .collect::<Vec<FileWrite>>()
    };

    match rayon::ThreadPoolBuilder::new()
        .num_threads(TAG_WRITE_THREADS)
        .build()
    {
        Ok(pool) => pool.install(map_files),
        Err(e) => {
            // Extremely unlikely; fall back to the global pool rather than
            // aborting the edit.
            log::warn!("tag-write pool build failed ({e}); using the global pool");
            map_files()
        }
    }
}

/// Land the successful writes in one transaction: resolve ids, refresh each
/// track row, and apply the artwork override the metadata UPDATE can't do.
/// Records per-file failures / unsupported fields into `report`.
async fn run_commit(
    db: &DbPool,
    files: &[FileWrite],
    edit: &TagEdit,
    cached_artwork: Option<&str>,
    report: &mut TagEditReport,
) -> Result<Vec<i64>, AppError> {
    let mut tx = db.write().begin().await?;
    let mut updated_ids: Vec<i64> = Vec::new();
    let mut album_ids: Vec<i64> = Vec::new();

    for f in files {
        let (meta, unsupported) = match &f.outcome {
            Ok(ok) => ok,
            Err(msg) => {
                report.failures.push((f.path.clone(), msg.clone()));
                continue;
            }
        };

        let path = Path::new(&f.path);
        let Some(rids) =
            queries::scan::resolve_track_context(&mut tx, path, &f.path, meta, "Tag edit").await?
        else {
            report
                .failures
                .push((f.path.clone(), "not in a library folder".to_owned()));
            continue;
        };

        queries::scan::update_track_metadata(&mut tx, &f.path, meta, &rids).await?;
        updated_ids.push(f.id);
        if !unsupported.is_empty() {
            report.unsupported.push((f.path.clone(), unsupported.clone()));
        }

        // Artwork the metadata UPDATE can't express: its `COALESCE(?, ...)` can
        // never null a path, and a re-extract of a Replaced file returns the
        // *external* cover (which shadows embedded art) rather than the one we
        // just embedded.
        match edit.artwork {
            ArtworkEdit::Replace => {
                if let Some(aid) = rids.album_id {
                    album_ids.push(aid);
                }
            }
            ArtworkEdit::Remove => {
                // The re-extracted value: an external `cover.jpg` if one exists,
                // else NULL. Album artwork is left alone — blanking a whole album
                // because one track's embedded art was removed would be wrong.
                queries::track::set_track_artwork(&mut tx, &[f.id], meta.artwork_path.as_deref())
                    .await?;
            }
            ArtworkEdit::Keep => {}
        }
    }

    // Replace: one authoritative overwrite to every updated track and its
    // album(s), so the Albums grid card updates too (the roll-up only fills
    // NULL rows). Skipped when the cache write failed — leaving the COALESCE'd
    // value beats nulling a cover we can't repoint.
    if edit.artwork == ArtworkEdit::Replace
        && !updated_ids.is_empty()
        && let Some(cached) = cached_artwork
    {
        queries::track::set_track_artwork(&mut tx, &updated_ids, Some(cached)).await?;
        album_ids.sort_unstable();
        album_ids.dedup();
        queries::album::set_album_artwork(&mut tx, &album_ids, Some(cached)).await?;
    }

    tx.commit().await?;
    Ok(updated_ids)
}

#[cfg(test)]
#[path = "tests/tags_tests.rs"]
mod tests;
