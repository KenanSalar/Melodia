use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use sqlx::AssertSqlSafe;

use crate::database::queries;
use crate::database::queries::artist::UNKNOWN_ARTIST_ID;
use crate::database::queries::scan::NameCache;
use crate::database::{DbPool, MAX_BINDS_PER_STATEMENT, values_placeholders};
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
    /// The freshly inserted tracks' ids, in **input order**.
    pub inserted_track_ids: Vec<i64>,
}

/// Rows a scan or watcher batch writes above which the stats triggers are dropped and the counts
/// rebuilt by one `recalculate_all_stats` at the end; for a handful of writes the triggers are
/// cheaper than that recalc.
pub(crate) const BULK_THRESHOLD: usize = 20;

/// What a scan's chunks wrote, summed across them, and how.
pub struct Ingested {
    /// The chunks commit with the stats triggers dropped, leaving the counts to the final recalc.
    bulk: bool,
    pub inserted: u32,
    pub(crate) moved: u32,
    pub(crate) updated: u32,
}

impl Ingested {
    /// A scan that will write `rows`, its parsed files and its orphans together.
    pub fn for_delta(rows: usize) -> Self {
        Self { bulk: rows > BULK_THRESHOLD, inserted: 0, moved: 0, updated: 0 }
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

/// The caches [`resolve_ids`] reuses within one ingest transaction. Artist and genre are in
/// [`NameCache`], which the join writers reach too.
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

/// Ingests scanned files within `tx`.
///
/// A new path whose hash matches a row whose file is gone re-points that row rather than
/// inserting, and an existing path whose size and mtime haven't moved is skipped. With
/// `update_artwork_on_existing`, a skipped row missing artwork takes the file's, which the library
/// scan wants and an import doesn't.
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

        // Consumed once re-pointed, so a second same-hash file inserts rather than stealing the
        // row; a failed folder resolution leaves it for a later one.
        if let Some((existing_id, old_path)) = moved_from.get(meta.file_hash.as_str()).cloned() {
            let Some(folder_id) =
                resolve_folder_id(tx, folder_resolution, file, file_path_str, &mut caches.folder)
                    .await?
            else {
                continue;
            };
            // Resolved inside this transaction, and nothing deletes ahead of this loop, so the
            // row is there.
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

    inserts.flush(tx, &mut caches.names).await?;
    flush_artwork_backfill(tx, artwork_backfill).await?;

    let inserted_track_ids = inserts.inserted_ids;
    Ok(IngestResult {
        inserted_count: u32::try_from(inserted_track_ids.len()).unwrap_or(u32::MAX),
        moved_count,
        updated_count,
        inserted_track_ids,
    })
}

/// The stored size and mtime of each scanned path that already has a row.
///
/// Chunked over `scanned_files` itself, so a chunk's `Cow` binds drop at its end rather than a
/// `String` per scanned path living for the whole ingest.
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
/// disk, a row whose path is still there being a duplicate the new file inserts beside.
///
/// The stat runs with the writer transaction open: moving it ahead would change how a concurrent
/// delete races it.
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
    let still_present = batch_still_on_disk(old_paths, pool).await;
    by_hash.retain(|_, (_, old_path)| !still_present.contains(old_path));
    Ok(by_hash)
}

fn file_name_of(file: &ScannedFile) -> String {
    file.path.file_name().and_then(|f| f.to_str()).unwrap_or("").to_string()
}

/// New-file inserts, flushed as multi-row statements a chunk at a time. Safe to defer: nothing
/// later in the loop reads a pending row, the lookups running against the pre-loaded maps and the
/// FK upserts immediately.
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

/// Below this the rayon overhead outweighs the stats, so the small batches incremental rescans
/// make walk sequentially.
const STAT_PAR_THRESHOLD: usize = 32;

/// The subset of `paths` still on disk under their own spelling, a batch past
/// [`STAT_PAR_THRESHOLD`] stat'ed on the scan pool from a blocking thread.
///
/// **Under their own spelling** because a case-insensitive or normalizing filesystem answers
/// `exists()` for a spelling no entry carries, and a rename that changed only case or normalization
/// is a move. Only the file name is asked: the folder root above it is spelled as the user added
/// it, so a directory renamed by case alone still reads as a copy.
async fn batch_still_on_disk(paths: Vec<String>, pool: &ScanPool) -> HashSet<String> {
    if paths.is_empty() {
        return HashSet::new();
    }
    if paths.len() < STAT_PAR_THRESHOLD {
        return listed_as_spelled(paths.into_iter().filter(|p| Path::new(p).exists()).collect());
    }
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let existing = pool.install(|| {
            paths.par_iter().filter(|p| Path::new(p).exists()).cloned().collect::<HashSet<String>>()
        });
        listed_as_spelled(existing)
    })
    .await
    .unwrap_or_default()
}

/// Keeps the paths their directory lists under exactly that name, reading each directory once. A
/// directory that can't be read keeps what `exists()` said.
fn listed_as_spelled(paths: HashSet<String>) -> HashSet<String> {
    let mut listings: HashMap<PathBuf, Option<HashSet<OsString>>> = HashMap::new();
    paths
        .into_iter()
        .filter(|path| {
            let path = Path::new(path);
            let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
                return true;
            };
            listings
                .entry(dir.to_path_buf())
                .or_insert_with(|| entry_names(dir))
                .as_ref()
                .is_none_or(|names| names.contains(name))
        })
        .collect()
}

fn entry_names(dir: &Path) -> Option<HashSet<OsString>> {
    let entries = std::fs::read_dir(dir).ok()?;
    Some(entries.flatten().map(|entry| entry.file_name()).collect())
}

/// Applies the queued artwork backfills as one `WITH v AS (VALUES …)` UPDATE per chunk of
/// `(file_path, artwork_path)` pairs, `batch_update_hashes`' shape.
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
        let rows = values_placeholders(chunk.len(), COLS_PER_ROW);
        let sql = format!(
            "WITH v(path, art) AS (VALUES {rows})
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

/// The file's `folder_id`, or `None` for a file with no parent directory, which is skipped.
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

/// A track's artist, album, genre and folder ids, or `None` where the file is skipped.
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

    // The **first** credited name, as `resolve_track_context` takes: a row keyed on the whole line
    // is an artist nobody recorded under.
    let artist_name = meta.artist.primary_name();
    let album_name = meta.album.as_deref().unwrap_or("");
    let genre_name = meta.genres.primary().unwrap_or("");

    let artist_id = caches.names.artist(tx, artist_name, UNKNOWN_ARTIST_ID).await?;

    // The album groups by its album artist, so a per-track featured credit doesn't split it.
    let album_artist_name = queries::scan::album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        caches.names.artist(tx, album_artist_name, UNKNOWN_ARTIST_ID).await?
    };

    let album_id = if let Some(&id) =
        caches.album.get(album_name).and_then(|by_artist| by_artist.get(&album_artist_id))
    {
        id
    } else {
        // Behind the cache miss: rewriting an album's credit per track would cost a delete and an
        // insert per row of a bulk scan.
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
