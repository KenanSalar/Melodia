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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;

use crate::database::{DbPool, queries};
use crate::entities::track::{TagEditRow, TrackSummary};
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

/// The rows that populate the Edit Track Information dialog.
///
/// Sits here rather than being called straight off `queries::track` by the
/// dialog's own wiring: the UI layer reaches the database through this module,
/// and [`apply_tag_edit`] below is the write half of the same feature.
pub async fn get_tag_edit_rows(
    state: &AppState,
    ids: &[i64],
) -> Result<Vec<TagEditRow>, AppError> {
    queries::track::get_tag_edit_rows_by_ids(&state.db, ids).await
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
        // Only pay for the refetch + resync when the player actually references
        // an edited track — the uncommon case. `sync_track_summaries` no-ops
        // otherwise, but the fetch + `HashMap` build run before its own check,
        // so gate them on the same cheap membership probe.
        let touched: HashSet<i64> = updated_ids.iter().copied().collect();
        if crate::player::state::any_tracked(&state.player_state, |id| touched.contains(&id)) {
            let fresh = queries::track::get_track_summaries_by_ids(&state.db, &updated_ids).await?;
            let map: HashMap<i64, TrackSummary> = fresh.into_iter().map(|t| (t.id, t)).collect();
            crate::player::state::sync_track_summaries(&state.player_state, &state.sinks, &map);
        }

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
            let (picture, cached_artwork) =
                prepare_artwork(&edit, artwork_source.as_deref(), &artwork_dir)?;

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

    let updated_ids = match run_commit(db, &files, edit, cached_artwork.as_deref(), &mut report).await
    {
        Ok(ids) => ids,
        Err(e) => {
            // Only on the (rare) tx failure: unmark every path we marked before
            // writing, so the watcher re-ingests instead of leaving the DB
            // permanently stale. Built here, not on the happy path, to avoid N
            // `PathBuf` allocations per successful commit.
            let marked: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(&f.path)).collect();
            self_writes.unmark(&marked);
            return Err(e);
        }
    };

    report.updated = updated_ids.len();
    Ok((report, updated_ids))
}

/// Decode + validate the picked cover and copy it into the content-addressed
/// artwork dir. `Keep` / `Remove` need neither, so return `(None, None)`.
///
/// Blocking (image decode + file copy) — call from inside the `spawn_blocking`
/// write pass. `cover_picture_from_path` decode-validates (lofty only sniffs 8
/// bytes) and re-encodes non-JPEG/PNG; `cache_image_file` copies the original
/// bytes for the DB `artwork_path`. Run FIRST — a corrupt pick fails the whole
/// edit here (via `?`), before any file is touched.
fn prepare_artwork(
    edit: &TagEdit,
    artwork_source: Option<&Path>,
    artwork_dir: &Path,
) -> Result<(Option<lofty::picture::Picture>, Option<String>), AppError> {
    if edit.artwork != ArtworkEdit::Replace {
        return Ok((None, None));
    }
    let source = artwork_source.ok_or_else(|| {
        AppError::metadata_msg("artwork Replace requested without a source image".to_owned())
    })?;
    let picture = tag_writer::cover_picture_from_path(source)?;
    let cached = artwork::cache_image_file(source, artwork_dir);
    Ok((Some(picture), cached))
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
    // On a Replace the re-extract's artwork is discarded: `apply_replace_artwork`
    // overwrites every row (and album) with the batch's single `cached_artwork`,
    // so reading the just-embedded cover back (a full decode + a redundant BLAKE3
    // pass + an artwork-dir write, per track, un-memoized) is pure waste. Skip it
    // for Replace only — `Remove` needs the external-cover fallback and `Keep`'s
    // COALESCE backfills a missing `artwork_path`.
    let skip_artwork = edit.artwork == ArtworkEdit::Replace;
    let map_files = || {
        rows.par_iter()
            .map(|(id, path)| {
                let p = Path::new(path);
                // Mark BEFORE the write, per file, so the TTL clock tracks the
                // event this write is about to fire — and with the DB `file_path`
                // (the set keys on exact `PathBuf` equality).
                self_writes.mark(p);
                let outcome = match tag_writer::apply_to_file(p, edit, picture) {
                    Ok(unsupported) => match extract_metadata(p, artwork_dir, cover_cache, skip_artwork) {
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

/// Cache key for `run_commit`'s FK-resolution memo. Holds everything
/// [`queries::scan::resolve_track_context`] derives its
/// [`queries::ResolvedIds`] from —
/// folder (via the parent dir, since folder lookup is a path-prefix match),
/// artist, album, album-artist (the album's grouping key), `year` (album upsert's
/// `COALESCE`-on-conflict input), and genre — so identical keys yield identical
/// ids. Keeping `year` in the key preserves the per-track album-year semantics:
/// tracks with differing years land in different buckets and each still upserts.
type ResolveKey = (PathBuf, String, String, String, Option<i32>, String);

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
    // FK resolution is identical for every track sharing a folder + artist +
    // album/year + genre (the whole-album batch — the flagship case), so resolve
    // each distinct tuple once instead of re-running a folder lookup + three
    // `INSERT … ON CONFLICT … RETURNING` upserts per track. Function-scoped, so
    // it drops at batch end (no persistent cache).
    let mut resolve_cache: HashMap<ResolveKey, queries::scan::ResolvedIds> = HashMap::new();
    // Batched artwork-Remove ids: the common `None` (cover removed, no external
    // fallback) case flushes as one `IN (…)` UPDATE after the loop; the rare
    // external-cover survivors keep their per-track value.
    let mut remove_null_ids: Vec<i64> = Vec::new();
    let mut remove_ext_ids: Vec<(i64, String)> = Vec::new();

    for f in files {
        let (meta, unsupported) = match &f.outcome {
            Ok(ok) => ok,
            Err(msg) => {
                report.failures.push((f.path.clone(), msg.clone()));
                continue;
            }
        };

        let path = Path::new(&f.path);
        // Mirror `resolve_track_context`'s own `as_deref().unwrap_or("")`
        // normalization so a cached key matches the ids it would have produced.
        let key: ResolveKey = (
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
            meta.artist.clone().unwrap_or_default(),
            meta.album.clone().unwrap_or_default(),
            meta.album_artist.clone().unwrap_or_default(),
            meta.year,
            meta.genre.clone().unwrap_or_default(),
        );
        let rids = if let Some(cached) = resolve_cache.get(&key) {
            *cached
        } else {
            let Some(resolved) =
                queries::scan::resolve_track_context(&mut tx, path, &f.path, meta, "Tag edit").await?
            else {
                report
                    .failures
                    .push((f.path.clone(), "not in a library folder".to_owned()));
                continue;
            };
            resolve_cache.insert(key, resolved);
            resolved
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
                // Collected here, flushed after the loop: the `None` majority
                // becomes one batched UPDATE instead of one per track.
                match meta.artwork_path.as_deref() {
                    Some(p) => remove_ext_ids.push((f.id, p.to_owned())),
                    None => remove_null_ids.push(f.id),
                }
            }
            ArtworkEdit::Keep => {}
        }
    }

    match edit.artwork {
        ArtworkEdit::Replace => {
            apply_replace_artwork(&mut tx, &updated_ids, &mut album_ids, cached_artwork).await?;
        }
        ArtworkEdit::Remove => {
            if !remove_null_ids.is_empty() {
                queries::track::set_track_artwork(&mut tx, &remove_null_ids, None).await?;
            }
            for (id, p) in &remove_ext_ids {
                queries::track::set_track_artwork(&mut tx, &[*id], Some(p)).await?;
            }
        }
        ArtworkEdit::Keep => {}
    }

    // Backfill album covers from their tracks (null-only, never an overwrite), so
    // retagging a track into a different album lets that album inherit the track's
    // existing artwork — the scan/import/reconcile paths already do this, but the
    // tag editor didn't, leaving a moved track's new album with a blank cover.
    queries::scan::update_album_artwork_from_tracks(&mut tx).await?;

    // Retagging a track into a different album or genre can strand its old album
    // (and that album's artist) or old genre with zero tracks; nothing else deletes
    // an emptied row, so sweep orphans here before committing.
    queries::scan::prune_orphans(&mut tx).await?;

    tx.commit().await?;
    Ok(updated_ids)
}

/// Replace: one authoritative overwrite to every updated track and its album(s),
/// so the Albums grid card updates too (the metadata roll-up only fills NULL
/// rows). No-op when nothing was updated, or when the cache write failed
/// (`cached_artwork` is `None`) — leaving the COALESCE'd value beats nulling a
/// cover we can't repoint.
async fn apply_replace_artwork(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    updated_ids: &[i64],
    album_ids: &mut Vec<i64>,
    cached_artwork: Option<&str>,
) -> Result<(), AppError> {
    if updated_ids.is_empty() {
        return Ok(());
    }
    let Some(cached) = cached_artwork else {
        return Ok(());
    };
    queries::track::set_track_artwork(tx, updated_ids, Some(cached)).await?;
    album_ids.sort_unstable();
    album_ids.dedup();
    queries::album::set_album_artwork(tx, album_ids, Some(cached)).await?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/tags_tests.rs"]
mod tests;
