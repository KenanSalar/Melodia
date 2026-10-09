use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use sqlx::AssertSqlSafe;

use crate::database::queries;
use crate::database::queries::artist::UNKNOWN_ARTIST_ID;
use crate::database::queries::scan::NameCache;
use crate::database::{DbPool, MAX_BINDS_PER_STATEMENT};
use crate::media::ingest::scan_pool::ScanPool;
use melodia_core::entities::scan::{ExtractedMetadata, ScannedFile};
use melodia_core::error::AppError;

/// How to resolve the `folder_id` for each track during ingest.
pub(crate) enum FolderResolution {
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

/// What a scan's chunks wrote, summed across them, and how.
pub struct Ingested {
    /// The chunks commit with the stats triggers dropped, leaving the counts to the final recalc.
    bulk: bool,
    pub inserted: u32,
    pub(crate) moved: u32,
    pub(crate) updated: u32,
}

impl Ingested {
    pub fn new(bulk: bool) -> Self {
        Self { bulk, inserted: 0, moved: 0, updated: 0 }
    }

    pub(crate) fn is_bulk(&self) -> bool {
        self.bulk
    }

    pub fn add(&mut self, chunk: &IngestResult) {
        self.inserted += chunk.inserted_count;
        self.moved += chunk.moved_count;
        self.updated += chunk.updated_count;
    }

    pub fn any(&self) -> bool {
        self.inserted > 0 || self.updated > 0 || self.moved > 0
    }

    pub fn rewrote_existing(&self) -> bool {
        self.moved > 0 || self.updated > 0
    }
}

/// One chunk of a folder scan, in a write transaction of its own.
///
/// On the bulk path the stats triggers are dropped and recreated *inside* that transaction, so a
/// crash never leaves them missing; the counts lag until [`queries::scan::commit_scan`] recalculates
/// them.
pub async fn commit_scan_chunk(
    db: &DbPool,
    scanned_files: &[ScannedFile],
    folder_id: i64,
    scan_timestamp: &str,
    pool: &ScanPool,
    ingested: &Ingested,
) -> Result<IngestResult, AppError> {
    let mut tx = db.write().begin().await?;
    if ingested.is_bulk() {
        queries::stats::disable_stats_triggers(&mut tx).await?;
    }
    let result = ingest_scanned_files(
        &mut tx,
        scanned_files,
        &FolderResolution::Fixed(folder_id),
        scan_timestamp,
        true,
        pool,
    )
    .await?;
    if ingested.is_bulk() {
        queries::stats::enable_stats_triggers(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(result)
}

/// Files dropped onto a playlist or the queue, each filed under the library folder its parent
/// directory belongs to, in one transaction with the album covers and stats brought up to date.
pub async fn commit_import(
    db: &DbPool,
    scanned_files: &[ScannedFile],
    scan_timestamp: &str,
    pool: &ScanPool,
) -> Result<IngestResult, AppError> {
    let mut tx = db.write().begin().await?;
    queries::stats::disable_stats_triggers(&mut tx).await?;
    let result = ingest_scanned_files(
        &mut tx,
        scanned_files,
        &FolderResolution::FromParentDir,
        scan_timestamp,
        false,
        pool,
    )
    .await?;
    queries::scan::update_album_artwork_from_tracks(&mut tx).await?;
    queries::stats::recalculate_all_stats(&mut tx).await?;
    queries::stats::enable_stats_triggers(&mut tx).await?;
    tx.commit().await?;
    Ok(result)
}

/// Stored file info for incremental scan comparison.
struct ExistingTrackInfo {
    file_size: Option<i64>,
    date_modified: Option<String>,
}

impl ExistingTrackInfo {
    /// Whether the stored row still describes the file, so its metadata write can be skipped.
    fn is_current(&self, meta: &ExtractedMetadata) -> bool {
        self.file_size == Some(meta.file_size)
            && self.date_modified.is_some()
            && self.date_modified.as_deref() == meta.date_modified.as_deref()
    }
}

/// In-memory caches reused across `resolve_ids` calls within a single ingest
/// transaction. Bundles them so per-call signatures don't balloon.
///
/// Artist and genre live in `queries::scan::NameCache` rather than here, because the join
/// writers ask the same questions once per credit per track and had no way to reach a cache
/// scoped to this call.
struct ResolveCaches {
    names: NameCache,
    album: HashMap<String, HashMap<i64, Option<i64>>>,
    folder: HashMap<String, i64>,
}

impl ResolveCaches {
    fn with_capacity_for(estimated: usize) -> Self {
        Self {
            names: NameCache::for_chunk(estimated),
            album: HashMap::with_capacity(estimated / 8 + 1),
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
/// - `pool`: the pass's own, which the moved-file stat fans out on.
pub(crate) async fn ingest_scanned_files(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scanned_files: &[ScannedFile],
    folder_resolution: &FolderResolution,
    scan_timestamp: &str,
    update_artwork_on_existing: bool,
    pool: &ScanPool,
) -> Result<IngestResult, AppError> {
    let mut caches = ResolveCaches::with_capacity_for(scanned_files.len());
    let existing_tracks = load_existing(tx, scanned_files).await?;
    let mut moved_from = resolve_move_candidates(tx, scanned_files, &existing_tracks, pool).await?;
    let mut inserts = InsertBuffer::new(scanned_files.len(), scan_timestamp);
    let mut moved_count: u32 = 0;
    let mut updated_count: u32 = 0;

    // Collect (file_path, artwork_path) for unchanged-but-missing-artwork
    // tracks instead of issuing one UPDATE per row. Single batched UPDATE
    // after the loop.
    let mut artwork_backfill: HashMap<String, Vec<String>> = HashMap::new();

    for file in scanned_files {
        let file_path_cow = file.path.to_string_lossy();
        let file_path_str: &str = file_path_cow.as_ref();
        let meta = &file.metadata;

        if let Some(existing) = existing_tracks.get(file_path_str) {
            if existing.is_current(meta) {
                if update_artwork_on_existing && let Some(ref art_path) = meta.artwork_path {
                    artwork_backfill
                        .entry(art_path.clone())
                        .or_default()
                        .push(file_path_str.to_owned());
                }
            } else if let Some(ids) =
                resolve_ids(tx, meta, folder_resolution, file, file_path_str, &mut caches).await?
            {
                queries::scan::update_track_metadata(
                    tx,
                    file_path_str,
                    meta,
                    &ids,
                    &mut caches.names,
                )
                .await?;
                updated_count += 1;
            }
            continue;
        }

        // The entry is consumed after a successful re-point so two same-hash
        // new files in one scan can't both steal the one existing row — the
        // second falls through to a fresh insert (mirrors `reconcile.rs`'s
        // consume-once moved-candidates map). A failed folder resolution
        // leaves the entry available for a later same-hash file.
        if let Some((existing_id, old_path)) = moved_from.get(meta.file_hash.as_str()).cloned() {
            let Some(folder_id) =
                resolve_folder_id(tx, folder_resolution, file, file_path_str, &mut caches.folder)
                    .await?
            else {
                continue;
            };
            // `moved_from` was resolved inside this transaction and
            // nothing deletes rows before this loop (orphan pruning runs
            // after ingest), so the re-point bool is vacuously true.
            let _repointed = queries::scan::update_track_location(
                tx,
                existing_id,
                file_path_str,
                &file_name_of(file),
                folder_id,
                meta.date_modified.as_deref(),
            )
            .await?;
            moved_from.remove(meta.file_hash.as_str());
            log::info!("Detected moved file: {old_path} -> {file_path_str}");
            moved_count += 1;
            continue;
        }

        let Some(ids) =
            resolve_ids(tx, meta, folder_resolution, file, file_path_str, &mut caches).await?
        else {
            continue;
        };
        let row = queries::scan::NewTrackRow {
            file_path: file_path_str.to_owned(),
            file_name: file_name_of(file),
            meta,
            ids,
        };
        inserts.push(tx, row, &mut caches.names).await?;
    }

    // Flush the insert remainder before the artwork backfill so the new
    // rows exist for any later same-transaction reads.
    inserts.flush(tx, &mut caches.names).await?;

    // Drain the artwork backfill: one chunked UPDATE per (artwork_path)
    // group, instead of one per affected track.
    flush_artwork_backfill(tx, artwork_backfill).await?;

    let inserted_track_ids = inserts.inserted_ids;
    Ok(IngestResult {
        inserted_count: u32::try_from(inserted_track_ids.len()).unwrap_or(u32::MAX),
        moved_count,
        updated_count,
        inserted_track_ids,
    })
}

/// Batch-load existing tracks with file info for incremental comparison.
/// Maps `file_path` -> (`file_size`, `date_modified`) for mtime+size gate.
///
/// Chunked over `scanned_files` itself, so a chunk's `Cow` binds (alloc-free for valid UTF-8,
/// the common case) drop at its end rather than one `String` per scanned path living for the
/// whole ingest.
async fn load_existing(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scanned_files: &[ScannedFile],
) -> Result<HashMap<String, ExistingTrackInfo>, AppError> {
    let mut existing_tracks = HashMap::with_capacity(scanned_files.len());
    for chunk in scanned_files.chunks(MAX_BINDS_PER_STATEMENT) {
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
            existing_tracks
                .insert(path, ExistingTrackInfo { file_size: size, date_modified: mtime });
        }
    }
    Ok(existing_tracks)
}

/// The existing rows a new path may have moved from, by hash: those whose own path is gone from
/// disk. A row whose path is still there is a duplicate, and its file falls through to an insert.
///
/// Every new path's hash goes in one chunked query, rather than a round-trip per file on a first
/// scan. The stat runs with the writer transaction still open: moving it ahead of the transaction
/// would change how a concurrent delete races it.
async fn resolve_move_candidates(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    scanned_files: &[ScannedFile],
    existing_tracks: &HashMap<String, ExistingTrackInfo>,
    pool: &ScanPool,
) -> Result<HashMap<String, (i64, String)>, AppError> {
    let new_path_hashes: Vec<&str> = scanned_files
        .iter()
        .filter(|f| !existing_tracks.contains_key(f.path.to_string_lossy().as_ref()))
        .map(|f| f.metadata.file_hash.as_str())
        .collect();
    let mut by_hash = queries::track::lowest_id_by_hash_on(tx, &new_path_hashes).await?;
    let old_paths: Vec<String> = by_hash.values().map(|(_, p)| p.clone()).collect();
    let still_present = batch_stat_existence(old_paths, pool).await;
    by_hash.retain(|_, (_, old_path)| !still_present.contains(old_path));
    Ok(by_hash)
}

fn file_name_of(file: &ScannedFile) -> String {
    file.path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_string()
}

/// New-file inserts, buffered and flushed as multi-row statements a chunk at a time, never
/// holding more than one chunk's worth of row metadata.
///
/// Safe to defer: nothing later in the ingest loop reads not-yet-inserted rows (path/hash lookups
/// run against the pre-loaded maps, and FK upserts in `resolve_ids` execute immediately).
struct InsertBuffer<'a> {
    pending: Vec<queries::scan::NewTrackRow<'a>>,
    scan_timestamp: &'a str,
    inserted_ids: Vec<i64>,
}

impl<'a> InsertBuffer<'a> {
    fn new(file_count: usize, scan_timestamp: &'a str) -> Self {
        Self {
            pending: Vec::with_capacity(queries::scan::INSERT_CHUNK_ROWS),
            scan_timestamp,
            inserted_ids: Vec::with_capacity(file_count),
        }
    }

    async fn push(
        &mut self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        row: queries::scan::NewTrackRow<'a>,
        names: &mut NameCache,
    ) -> Result<(), AppError> {
        self.pending.push(row);
        if self.pending.len() >= queries::scan::INSERT_CHUNK_ROWS {
            self.flush(tx, names).await?;
        }
        Ok(())
    }

    async fn flush(
        &mut self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        names: &mut NameCache,
    ) -> Result<(), AppError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let ids = queries::scan::insert_tracks_batch(tx, &self.pending, self.scan_timestamp, names)
            .await?;
        self.inserted_ids.extend(ids);
        self.pending.clear();
        Ok(())
    }
}

/// Below this threshold the rayon thread-pool overhead dominates the
/// savings — small move-detection batches (the common case during
/// incremental rescans) walk sequentially. Per `.claude/rules/rayon.md`.
const STAT_PAR_THRESHOLD: usize = 32;

/// Run `Path::exists()` over `paths`, returning the subset that's actually present on disk. A
/// batch past [`STAT_PAR_THRESHOLD`] fans out on the scan pool from a blocking thread, keeping the
/// syscalls off the async worker; the caller's transaction stays open across the await either way.
async fn batch_stat_existence(paths: Vec<String>, pool: &ScanPool) -> HashSet<String> {
    if paths.is_empty() {
        return HashSet::new();
    }
    if paths.len() < STAT_PAR_THRESHOLD {
        return paths.into_iter().filter(|p| std::path::Path::new(p).exists()).collect();
    }
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        pool.install(|| {
            paths
                .par_iter()
                .filter(|p| std::path::Path::new(p).exists())
                .cloned()
                .collect::<HashSet<String>>()
        })
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
    let chunk_size = MAX_BINDS_PER_STATEMENT / COLS_PER_ROW;

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
    meta: &ExtractedMetadata,
    folder_resolution: &FolderResolution,
    file: &ScannedFile,
    file_path_str: &str,
    caches: &mut ResolveCaches,
) -> Result<Option<queries::ResolvedIds>, AppError> {
    let Some(folder_id) =
        resolve_folder_id(tx, folder_resolution, file, file_path_str, &mut caches.folder).await?
    else {
        return Ok(None);
    };

    // The **first** credited name, matching `queries::scan::resolve_track_context` — an `artists`
    // row keyed on the whole credit line is a third artist nobody recorded under.
    let artist_name = meta.artist.primary_name();
    let album_name = meta.album.as_deref().unwrap_or("");
    let genre_name = meta.genres.primary().unwrap_or("");

    let artist_id = caches.names.artist(tx, artist_name, UNKNOWN_ARTIST_ID).await?;

    // The album-artist (album_artist tag, else the track artist). The album groups by this, so a
    // per-track featured credit doesn't split it.
    let album_artist_name = queries::scan::album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        caches.names.artist(tx, album_artist_name, UNKNOWN_ARTIST_ID).await?
    };

    // Resolve album (two-level cache, keyed on the album-artist)
    let album_id = if let Some(&id) =
        caches.album.get(album_name).and_then(|by_artist| by_artist.get(&album_artist_id))
    {
        id
    } else {
        // Behind the cache miss deliberately: an album's credit is a property of the album, so
        // rewriting it per *track* would cost a delete-and-insert cycle per row of a bulk scan.
        let album_credit = queries::scan::album_credit_for(meta);
        let id = queries::scan::upsert_album(
            tx,
            album_name,
            album_artist_id,
            &album_credit,
            meta,
            &mut caches.names,
        )
        .await?;
        caches.album.entry(album_name.to_owned()).or_default().insert(album_artist_id, id);
        id
    };

    let genre_id = caches.names.genre(tx, genre_name).await?;

    Ok(Some(queries::ResolvedIds { artist_id, album_id, genre_id, folder_id }))
}

#[cfg(test)]
#[path = "tests/ingest_tests.rs"]
mod tests;
