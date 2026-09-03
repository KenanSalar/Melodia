use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use sqlx::AssertSqlSafe;

use crate::database::SQLITE_BIND_LIMIT;
use crate::database::queries;
use crate::entities::scan::ScannedFile;
use crate::error::AppError;

/// Sentinel artist ID for tracks/albums with no known artist.
/// Matches the row inserted in schema.sql.
const UNKNOWN_ARTIST_ID: i64 = 1;

/// How to resolve the `folder_id` for each track during ingest.
pub enum FolderResolution {
    /// All tracks belong to the same folder (library scan).
    Fixed(i64),
    /// Resolve folder from each file's parent directory, with caching (file import).
    FromParentDir,
}

/// Result of ingesting scanned files into the database.
pub struct IngestResult {
    pub inserted_count: u32,
    pub moved_count: u32,
    pub updated_count: u32,
    /// IDs of the freshly-inserted tracks, in **input order**. Populated by
    /// `insert_tracks_batch`'s `RETURNING id, file_path` (remapped to input
    /// order via the returned path, since `RETURNING` output order is
    /// unspecified) so callers don't need a follow-up `WHERE file_path IN
    /// (…)` lookup.
    pub inserted_track_ids: Vec<i64>,
}

/// Stored file info for incremental scan comparison.
struct ExistingTrackInfo {
    file_size: Option<i64>,
    date_modified: Option<String>,
}

/// In-memory caches reused across `resolve_ids` calls within a single ingest
/// transaction. Bundles the four parallel `HashMap`s so per-call signatures
/// don't balloon (and so capacity hints stay co-located).
struct ResolveCaches {
    artist: HashMap<String, i64>,
    album: HashMap<String, HashMap<i64, Option<i64>>>,
    genre: HashMap<String, Option<i64>>,
    folder: HashMap<String, i64>,
}

impl ResolveCaches {
    fn with_capacity_for(estimated: usize) -> Self {
        Self {
            artist: HashMap::with_capacity(estimated / 10 + 1),
            album: HashMap::with_capacity(estimated / 8 + 1),
            genre: HashMap::with_capacity(32),
            folder: HashMap::with_capacity(estimated / 20 + 1),
        }
    }
}

/// Insert scanned media files into the database within the given transaction.
/// Deduplicates artist/album/genre upserts via in-memory caches.
///
/// Features:
/// - **Moved file detection**: If a new path has a hash matching an existing track,
///   the existing track's path is updated instead of inserting a duplicate.
/// - **Incremental scanning**: If an existing path's mtime + `file_size` haven't changed,
///   the file is skipped entirely. If they have changed, metadata is re-extracted.
/// - `update_artwork_on_existing`: when `true`, updates `artwork_path` on tracks that
///   already exist but have no artwork (used by library scan, not by file import).
pub async fn ingest_scanned_files(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scanned_files: &[ScannedFile],
    folder_resolution: &FolderResolution,
    scan_timestamp: &str,
    update_artwork_on_existing: bool,
) -> Result<IngestResult, AppError> {
    let estimated = scanned_files.len();
    let mut caches = ResolveCaches::with_capacity_for(estimated);
    let mut inserted_count: u32 = 0;
    let mut moved_count: u32 = 0;
    let mut updated_count: u32 = 0;
    let mut inserted_track_ids: Vec<i64> = Vec::with_capacity(scanned_files.len());
    // New-file inserts are buffered and flushed as multi-row statements —
    // never holds more than one chunk's worth of row metadata.
    let mut pending_inserts: Vec<queries::scan::NewTrackRow<'_>> =
        Vec::with_capacity(queries::scan::INSERT_CHUNK_ROWS);

    // Batch-load existing tracks with file info for incremental comparison.
    // Maps file_path -> (file_size, date_modified) for mtime+size gate.
    //
    // Chunked over `scanned_files` directly — per-chunk `Vec<Cow<'_, str>>`
    // binds are alloc-free for valid-UTF-8 paths (the common case) and drop
    // at end of chunk, vs. the previous `Vec<String>` covering every scanned
    // file held resident for the whole function (~1 MiB on a 10k-track scan).
    let mut existing_tracks: HashMap<String, ExistingTrackInfo> =
        HashMap::with_capacity(scanned_files.len());
    for chunk in scanned_files.chunks(SQLITE_BIND_LIMIT) {
        let placeholders = crate::database::placeholders(chunk.len());
        let sql = format!(
            "SELECT file_path, file_size, date_modified FROM tracks WHERE file_path IN ({placeholders})"
        );
        let mut query =
            sqlx::query_as::<_, (String, Option<i64>, Option<String>)>(AssertSqlSafe(sql));
        let path_cows: Vec<Cow<'_, str>> = chunk.iter().map(|f| f.path.to_string_lossy()).collect();
        for cow in &path_cows {
            query = query.bind(cow.as_ref());
        }
        let rows = query.persistent(false).fetch_all(&mut **tx).await?;
        for (path, size, mtime) in rows {
            existing_tracks.insert(
                path,
                ExistingTrackInfo {
                    file_size: size,
                    date_modified: mtime,
                },
            );
        }
    }

    // Batch-load (file_hash → (id, file_path)) for every "new path" file in
    // one shot. Replaces a per-file `find_track_by_hash` query inside the hot
    // loop (was O(new_files) round-trips on a fresh first scan).
    let new_path_hashes: Vec<&str> = scanned_files
        .iter()
        .filter(|f| !existing_tracks.contains_key(f.path.to_string_lossy().as_ref()))
        .map(|f| f.metadata.file_hash.as_str())
        .collect();
    let mut hash_to_existing = batch_lookup_by_hash(tx, &new_path_hashes).await?;

    // For every (existing_id, old_path) candidate above, stat the old path
    // off-thread so the writer transaction isn't blocked by a syscall per
    // row. The `existing_old_paths_present` set holds the subset that still
    // exists on disk — those are duplicates and fall through to insert; the
    // rest are treated as moves.
    let candidate_old_paths: Vec<String> =
        hash_to_existing.values().map(|(_, p)| p.clone()).collect();
    let existing_old_paths_present = batch_stat_existence(candidate_old_paths).await;

    // Collect (file_path, artwork_path) for unchanged-but-missing-artwork
    // tracks instead of issuing one UPDATE per row. Single batched UPDATE
    // after the loop.
    let mut artwork_backfill: HashMap<String, Vec<String>> = HashMap::new();

    for file in scanned_files {
        let file_path_cow = file.path.to_string_lossy();
        let file_path_str: &str = file_path_cow.as_ref();
        let meta = &file.metadata;

        // --- Existing path: check if file has changed ---
        if let Some(existing) = existing_tracks.get(file_path_str) {
            let unchanged = existing.file_size == Some(meta.file_size)
                && existing.date_modified.is_some()
                && existing.date_modified.as_deref() == meta.date_modified.as_deref();

            if unchanged {
                // File unchanged — skip metadata write, queue an artwork
                // backfill if the existing row is missing artwork.
                if update_artwork_on_existing && let Some(ref art_path) = meta.artwork_path {
                    artwork_backfill
                        .entry(art_path.clone())
                        .or_default()
                        .push(file_path_str.to_owned());
                }
                continue;
            }

            // File changed — resolve IDs and update metadata
            let Some((artist_id, album_id, genre_id, folder_id)) =
                resolve_ids(tx, meta, folder_resolution, file, file_path_str, &mut caches).await?
            else {
                continue;
            };

            let ids = queries::ResolvedIds {
                artist_id,
                album_id,
                genre_id,
                folder_id,
            };

            queries::scan::update_track_metadata(tx, file_path_str, meta, &ids).await?;
            updated_count += 1;
            continue;
        }

        // --- New path: check for moved file (same hash, different path) ---
        // Only treat as a move if the old path no longer exists on disk —
        // pre-computed in `existing_old_paths_present` above. The entry is
        // consumed after a successful re-point so two same-hash new files
        // in one scan can't both steal the one existing row — the second
        // falls through to a fresh insert (mirrors `reconcile.rs`'s
        // consume-once moved-candidates map). A failed folder resolution
        // leaves the entry available for a later same-hash file.
        if let Some((existing_id, old_path)) =
            hash_to_existing.get(meta.file_hash.as_str()).cloned()
            && !existing_old_paths_present.contains(&old_path)
        {
            let Some(folder_id) =
                resolve_folder_id(tx, folder_resolution, file, file_path_str, &mut caches.folder)
                    .await?
            else {
                continue;
            };

            let file_name =
                file.path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_string();

            // `hash_to_existing` was resolved inside this transaction and
            // nothing deletes rows before this loop (orphan pruning runs
            // after ingest), so the re-point bool is vacuously true.
            let _repointed = queries::scan::update_track_location(
                tx,
                existing_id,
                file_path_str,
                &file_name,
                folder_id,
                meta.date_modified.as_deref(),
            )
            .await?;
            hash_to_existing.remove(meta.file_hash.as_str());
            log::info!("Detected moved file: {old_path} -> {file_path_str}");
            moved_count += 1;
            continue;
        }

        // --- Truly new file: insert (buffered) ---
        let Some((artist_id, album_id, genre_id, folder_id)) =
            resolve_ids(tx, meta, folder_resolution, file, file_path_str, &mut caches).await?
        else {
            continue;
        };

        let ids = queries::ResolvedIds {
            artist_id,
            album_id,
            genre_id,
            folder_id,
        };

        let file_name = file.path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_string();

        // Buffer instead of executing one 43-bind INSERT per file —
        // `insert_tracks_batch` flushes a full chunk as a single
        // multi-row statement (~27× fewer round-trips on a fresh scan).
        // Safe to defer: nothing later in this loop reads not-yet-
        // inserted rows (path/hash lookups run against the pre-loaded
        // maps, and FK upserts in `resolve_ids` execute immediately).
        pending_inserts.push(queries::scan::NewTrackRow {
            file_path: file_path_str.to_owned(),
            file_name,
            meta,
            ids,
        });
        if pending_inserts.len() >= queries::scan::INSERT_CHUNK_ROWS {
            let ids =
                queries::scan::insert_tracks_batch(tx, &pending_inserts, scan_timestamp).await?;
            inserted_count += u32::try_from(ids.len()).unwrap_or(u32::MAX);
            inserted_track_ids.extend(ids);
            pending_inserts.clear();
        }
    }

    // Flush the insert remainder before the artwork backfill so the new
    // rows exist for any later same-transaction reads.
    if !pending_inserts.is_empty() {
        let ids = queries::scan::insert_tracks_batch(tx, &pending_inserts, scan_timestamp).await?;
        inserted_count += u32::try_from(ids.len()).unwrap_or(u32::MAX);
        inserted_track_ids.extend(ids);
        pending_inserts.clear();
    }

    // Drain the artwork backfill: one chunked UPDATE per (artwork_path)
    // group, instead of one per affected track.
    flush_artwork_backfill(tx, artwork_backfill).await?;

    Ok(IngestResult {
        inserted_count,
        moved_count,
        updated_count,
        inserted_track_ids,
    })
}

/// Chunked `WHERE file_hash IN (…)` lookup, deduped to the lowest-id row per
/// hash to match `find_track_by_hash`'s `ORDER BY id ASC LIMIT 1` semantics.
async fn batch_lookup_by_hash(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    hashes: &[&str],
) -> Result<HashMap<String, (i64, String)>, AppError> {
    let mut out: HashMap<String, (i64, String)> = HashMap::new();
    if hashes.is_empty() {
        return Ok(out);
    }
    // Dedup so the bind list isn't quadratic on large duplicate-content sets.
    let unique: HashSet<&str> = hashes.iter().copied().collect();
    let unique: Vec<&str> = unique.into_iter().collect();

    for chunk in unique.chunks(SQLITE_BIND_LIMIT) {
        let placeholders = crate::database::placeholders(chunk.len());
        // ORDER BY id ASC + entry().or_insert keeps the lowest-id row per
        // hash, matching the singleton query's behaviour.
        let sql = format!(
            "SELECT file_hash, id, file_path FROM tracks
             WHERE file_hash IN ({placeholders})
             ORDER BY id ASC"
        );
        let mut q = sqlx::query_as::<_, (String, i64, String)>(AssertSqlSafe(sql));
        for h in chunk {
            q = q.bind(*h);
        }
        let rows = q.persistent(false).fetch_all(&mut **tx).await?;
        for (h, id, path) in rows {
            out.entry(h).or_insert((id, path));
        }
    }
    Ok(out)
}

/// Run `Path::exists()` over `paths` in parallel on a blocking thread pool,
/// returning the subset that's actually present on disk. Lifts the syscall
/// out of the writer transaction so the writer connection isn't held while
/// the kernel walks inodes.
/// Below this threshold the rayon thread-pool overhead dominates the
/// savings — small move-detection batches (the common case during
/// incremental rescans) walk sequentially. Per `.claude/rules/rayon.md`.
const STAT_PAR_THRESHOLD: usize = 32;

async fn batch_stat_existence(paths: Vec<String>) -> HashSet<String> {
    if paths.is_empty() {
        return HashSet::new();
    }
    if paths.len() < STAT_PAR_THRESHOLD {
        return paths.into_iter().filter(|p| std::path::Path::new(p).exists()).collect();
    }
    tokio::task::spawn_blocking(move || {
        paths
            .par_iter()
            .filter(|p| std::path::Path::new(p).exists())
            .cloned()
            .collect::<HashSet<String>>()
    })
    .await
    .unwrap_or_default()
}

/// Apply queued artwork backfills as a single chunked CTE-driven UPDATE.
/// Flattens the by-artwork-path grouping into `(file_path, art_path)` pairs
/// so we issue one UPDATE per chunk (~499 pairs each), not one UPDATE per
/// unique artwork. Mirrors the `WITH v AS (VALUES ...)` pattern in
/// `batch_update_hashes`. Round-trip count is O(chunks), not
/// O(unique artwork paths × file-path chunks).
async fn flush_artwork_backfill(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    by_artwork: HashMap<String, Vec<String>>,
) -> Result<(), AppError> {
    const COLS_PER_ROW: usize = 2;
    let chunk_size = SQLITE_BIND_LIMIT / COLS_PER_ROW;

    let mut pairs: Vec<(String, String)> = Vec::new();
    for (art_path, paths) in by_artwork {
        pairs.reserve(paths.len());
        for p in paths {
            pairs.push((p, art_path.clone()));
        }
    }
    if pairs.is_empty() {
        return Ok(());
    }

    for chunk in pairs.chunks(chunk_size) {
        let row_placeholders =
            std::iter::repeat_n("(?,?)", chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "WITH v(path, art) AS (VALUES {row_placeholders})
             UPDATE tracks
                SET artwork_path = (SELECT art FROM v WHERE v.path = tracks.file_path)
              WHERE file_path IN (SELECT path FROM v)
                AND (artwork_path IS NULL OR artwork_path = '')"
        );
        let mut q = sqlx::query(AssertSqlSafe(sql)).persistent(false);
        for (path, art) in chunk {
            q = q.bind(path).bind(art);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Resolve the `folder_id` for a file based on the folder resolution strategy.
/// Returns `None` when the file has no parent directory and should be skipped.
async fn resolve_folder_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    folder_resolution: &FolderResolution,
    file: &ScannedFile,
    file_path_str: &str,
    folder_cache: &mut HashMap<String, i64>,
) -> Result<Option<i64>, AppError> {
    match folder_resolution {
        FolderResolution::Fixed(id) => Ok(Some(*id)),
        FolderResolution::FromParentDir => {
            let parent_dir =
                file.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            if parent_dir.is_empty() {
                log::warn!("Skipping file with no parent directory: {file_path_str}");
                return Ok(None);
            }
            if let Some(&id) = folder_cache.get(&parent_dir) {
                Ok(Some(id))
            } else {
                let id = queries::folder::upsert_folder(tx, &parent_dir).await?;
                folder_cache.insert(parent_dir, id);
                Ok(Some(id))
            }
        }
    }
}

/// Resolve artist, album, genre, and folder IDs for a track, using caches.
/// Returns `None` when the file should be skipped (e.g. no parent directory).
async fn resolve_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    meta: &crate::entities::scan::ExtractedMetadata,
    folder_resolution: &FolderResolution,
    file: &ScannedFile,
    file_path_str: &str,
    caches: &mut ResolveCaches,
) -> Result<Option<(i64, Option<i64>, Option<i64>, i64)>, AppError> {
    let Some(folder_id) =
        resolve_folder_id(tx, folder_resolution, file, file_path_str, &mut caches.folder).await?
    else {
        return Ok(None);
    };

    let artist_name = meta.artist.as_deref().unwrap_or("");
    let album_name = meta.album.as_deref().unwrap_or("");
    let genre_name = meta.genre.as_deref().unwrap_or("");

    // Resolve artist
    let artist_id = if let Some(&id) = caches.artist.get(artist_name) {
        id
    } else {
        let id = queries::scan::upsert_artist(tx, artist_name, UNKNOWN_ARTIST_ID).await?;
        caches.artist.insert(artist_name.to_owned(), id);
        id
    };

    // Resolve the album-artist (album_artist tag, else the track artist) — the album
    // groups by this so a per-track featured credit doesn't split it. Reuses the
    // artist cache.
    let album_artist_name =
        meta.album_artist.as_deref().filter(|s| !s.is_empty()).unwrap_or(artist_name);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else if let Some(&id) = caches.artist.get(album_artist_name) {
        id
    } else {
        let id = queries::scan::upsert_artist(tx, album_artist_name, UNKNOWN_ARTIST_ID).await?;
        caches.artist.insert(album_artist_name.to_owned(), id);
        id
    };

    // Resolve album (two-level cache, keyed on the album-artist)
    let album_id = if let Some(&id) =
        caches.album.get(album_name).and_then(|by_artist| by_artist.get(&album_artist_id))
    {
        id
    } else {
        let id = queries::scan::upsert_album(tx, album_name, album_artist_id, meta.year).await?;
        caches.album.entry(album_name.to_owned()).or_default().insert(album_artist_id, id);
        id
    };

    // Resolve genre
    let genre_id = if let Some(&id) = caches.genre.get(genre_name) {
        id
    } else {
        let id = queries::scan::upsert_genre(tx, genre_name).await?;
        caches.genre.insert(genre_name.to_owned(), id);
        id
    };

    Ok(Some((artist_id, album_id, genre_id, folder_id)))
}

#[cfg(test)]
#[path = "tests/ingest_tests.rs"]
mod tests;
