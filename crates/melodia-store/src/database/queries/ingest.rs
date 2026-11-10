use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rusqlite::Transaction;

use crate::database::queries;
use crate::database::queries::artist::UNKNOWN_ARTIST_ID;
use crate::database::queries::scan::{
    CueWrites, Links, MoveCandidates, NameCache, SheetWriter, TrackFolder, file_name_of,
};
use crate::database::queries::track::file_rows::{LINKED_STAMP, WHOLE_FILE};
use crate::database::sql::Sql;
use crate::database::{DbPool, MAX_BINDS_PER_STATEMENT, placeholders, values_placeholders};
use crate::media::cue::CueChunk;
use crate::media::ingest::scan_pool::ScanPool;
use melodia_core::entities::scan::{ExtractedMetadata, FileStamp, ScannedFile};
use melodia_core::error::AppError;

/// Who is ingesting, which decides where each file is filed and what a kept-out file means to it.
pub(crate) enum IngestSource {
    /// A library folder's scan, every file filed under it. A kept-out file stays out, and a
    /// current row missing artwork takes the file's.
    FolderScan(i64),
    /// Files the user named, each taken as named and filed under the library folder holding it,
    /// else under a disabled folder made for its own directory ([`TrackFolder::Outside`]).
    Import,
}

impl IngestSource {
    fn keeps_out_excluded(&self) -> bool {
        matches!(self, Self::FolderScan(_))
    }

    fn backfills_artwork(&self) -> bool {
        matches!(self, Self::FolderScan(_))
    }

    /// The folder a file or a cue sheet at `path` is filed under, or `None` for one with no parent
    /// directory, which is skipped. A scan reads only a library folder (`library::scan`).
    fn folder_of(
        &self,
        tx: &Transaction<'_>,
        path: &Path,
        folder_cache: &mut HashMap<String, TrackFolder>,
    ) -> Result<Option<TrackFolder>, AppError> {
        match self {
            Self::FolderScan(id) => Ok(Some(TrackFolder::Library(*id))),
            Self::Import => import_folder_of(tx, path, folder_cache),
        }
    }
}

/// Result of ingesting scanned files into the database.
pub struct IngestResult {
    pub inserted_count: u32,
    pub moved_count: u32,
    pub updated_count: u32,
    /// The freshly inserted whole-file rows' ids, in **input order**.
    pub inserted_track_ids: Vec<i64>,
}

impl IngestResult {
    fn add_sheets(&mut self, sheets: &CueWrites) {
        self.inserted_count += sheets.inserted;
        self.updated_count += sheets.updated;
        self.moved_count += sheets.moved;
    }
}

/// Rows a batch writes above which the stats triggers are dropped and the counts rebuilt by one
/// `recalculate_all_stats` at the end; for a handful of writes the triggers are cheaper than that
/// recalc.
pub(crate) const BULK_THRESHOLD: usize = 20;

pub(crate) fn is_bulk(rows: usize) -> bool {
    rows > BULK_THRESHOLD
}

/// What a scan's chunks wrote, summed across them, and how.
#[derive(Clone, Copy)]
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
        Self { bulk: is_bulk(rows), inserted: 0, moved: 0, updated: 0 }
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

/// Files read for one commit, and the cue sheet jobs whose images are among them.
pub struct ScanChunk {
    pub files: Vec<ScannedFile>,
    pub cue: CueChunk,
}

/// One chunk of a folder scan, in a write transaction of its own.
///
/// On the bulk path the stats triggers are dropped and recreated *inside* that transaction, so a
/// crash never leaves them missing; the counts lag until [`queries::scan::commit_scan`] recalculates
/// them.
pub async fn commit_scan_chunk(
    db: &DbPool,
    chunk: ScanChunk,
    folder_id: i64,
    scan_timestamp: String,
    pool: ScanPool,
    ingested: &Ingested,
) -> Result<IngestResult, AppError> {
    let is_bulk = ingested.is_bulk();
    db.transaction(move |tx| {
        if is_bulk {
            queries::stats::disable_stats_triggers(tx)?;
        }
        let source = IngestSource::FolderScan(folder_id);
        let result = ingest_chunk(tx, chunk, &source, &scan_timestamp, &pool)?;
        if is_bulk {
            queries::stats::enable_stats_triggers(tx)?;
        }
        Ok(result)
    })
    .await
}

/// Files the user opened, dropped or brought back, filed as [`IngestSource::Import`] says, in one
/// transaction with the album covers and stats brought up to date. A kept-out file among them comes
/// back, the user having named it or a sheet naming it.
pub async fn commit_import(
    db: &DbPool,
    chunk: ScanChunk,
    scan_timestamp: String,
    pool: ScanPool,
) -> Result<IngestResult, AppError> {
    let bulk = is_bulk(chunk.files.len());
    db.transaction(move |tx| {
        let paths: Vec<Cow<'_, str>> =
            chunk.files.iter().map(|f| f.path.to_string_lossy()).collect();
        let paths: Vec<&str> = paths.iter().map(AsRef::as_ref).collect();
        queries::excluded_files::lift_on(tx, &paths)?;
        if bulk {
            queries::stats::disable_stats_triggers(tx)?;
        }
        let result = ingest_chunk(tx, chunk, &IngestSource::Import, &scan_timestamp, &pool)?;
        queries::scan::update_album_artwork_from_tracks(tx)?;
        if bulk {
            queries::stats::recalculate_all_stats(tx)?;
            queries::stats::enable_stats_triggers(tx)?;
        }
        Ok(result)
    })
    .await
}

/// Ingests `chunk` within `tx`, each file and sheet filed as `source` says. An image a sheet claims
/// goes to its sheet's job and never to the ordinary ingest, which runs first and writes the
/// whole-file row an image given back takes its playlist places onto.
fn ingest_chunk(
    tx: &Transaction<'_>,
    chunk: ScanChunk,
    source: &IngestSource,
    scan_timestamp: &str,
    pool: &ScanPool,
) -> Result<IngestResult, AppError> {
    let ScanChunk { files, cue } = chunk;
    let (claimed, unclaimed): (Vec<_>, Vec<_>) = if cue.is_empty() {
        (Vec::new(), files)
    } else {
        files.into_iter().partition(|file| cue.claims(&file.path))
    };
    let mut result = ingest_scanned_files(tx, &unclaimed, source, scan_timestamp, pool)?;
    let mut moved = image_move_candidates(tx, &claimed, pool)?;
    let mut sheets = SheetWriter::new(tx, &claimed, scan_timestamp, &mut moved);
    let mut folders = HashMap::new();
    for job in cue.jobs() {
        if let Some(folder) = source.folder_of(tx, &job.path, &mut folders)? {
            sheets.apply(job, folder)?;
        }
    }
    result.add_sheets(&sheets.into_writes());
    Ok(result)
}

/// The caches [`resolve_ids`] reuses within one ingest transaction. Artist and genre are in
/// [`NameCache`], which the join writers reach too.
struct ResolveCaches {
    names: NameCache,
    album: HashMap<String, HashMap<i64, Option<i64>>>,
    folder: HashMap<String, TrackFolder>,
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
/// inserting, and an existing path whose size and mtime haven't moved is skipped. A folder scan
/// passes a kept-out file by, restamping it with what it read, and re-points the exclusion of one
/// that moved. A row with the file's own name goes first, a kept-out file with it next and any
/// other row last: a moved keeper beside its kept-out twin must get its own row back.
pub(crate) fn ingest_scanned_files(
    tx: &Transaction<'_>,
    scanned_files: &[ScannedFile],
    source: &IngestSource,
    scan_timestamp: &str,
    pool: &ScanPool,
) -> Result<IngestResult, AppError> {
    let mut caches = ResolveCaches::with_capacity_for(scanned_files.len());
    let existing_tracks = load_existing(tx, scanned_files)?;
    let kept_out = kept_out(tx, source, scanned_files)?;
    let mut moved_from = resolve_move_candidates(tx, scanned_files, &existing_tracks, pool)?;
    if !source.keeps_out_excluded() {
        moved_from.forget_exclusions();
    }
    let mut inserts = InsertBuffer::new(scanned_files.len(), scan_timestamp);
    let mut moved_count: u32 = 0;
    let mut updated_count: u32 = 0;
    let mut artwork_backfill: HashMap<String, Vec<String>> = HashMap::new();
    let mut kept_out_read: Vec<(&Path, &ExtractedMetadata)> = Vec::new();

    for file in scanned_files {
        let file_path_cow = file.path.to_string_lossy();
        let file_path_str: &str = file_path_cow.as_ref();
        let meta = &file.metadata;

        if let Some(stamp) = existing_tracks.get(file_path_str) {
            if stamp.matches(meta.file_size, meta.date_modified.as_deref()) {
                if source.backfills_artwork()
                    && let Some(ref art_path) = meta.artwork_path
                {
                    artwork_backfill
                        .entry(art_path.clone())
                        .or_default()
                        .push(file_path_str.to_owned());
                }
            } else if let Some(ids) = resolve_ids(tx, meta, source, file, &mut caches)? {
                let updated = queries::scan::update_track_metadata(
                    tx,
                    file_path_str,
                    meta,
                    &ids,
                    &mut caches.names,
                )?;
                if updated {
                    updated_count += 1;
                }
            }
            continue;
        }

        if kept_out.contains(file_path_str) {
            kept_out_read.push((&file.path, meta));
            continue;
        }

        // Spent once re-pointed, so a second same-hash file can't claim the same row; a failed
        // folder resolution leaves it for a later one.
        if let Some((existing_id, old_path)) =
            moved_from.track_for(meta.file_hash.as_str(), &file.path)
        {
            let Some(folder) = source.folder_of(tx, &file.path, &mut caches.folder)? else {
                continue;
            };
            // Resolved inside this transaction, and nothing deletes ahead of this loop, so the
            // row is there.
            let _repointed = queries::scan::update_track_location(
                tx,
                &old_path,
                file_path_str,
                &file_name_of(&file.path),
                folder.id(),
                meta.date_modified.as_deref(),
            )?;
            if let TrackFolder::Outside(_) = folder {
                queries::outside::unlink_on(tx, &[existing_id])?;
            }
            moved_from.spend_track(meta.file_hash.as_str(), existing_id);
            log::info!("Detected moved file: {old_path} -> {file_path_str}");
            moved_count += 1;
            continue;
        }

        if let Some((exclusion_id, old_path)) = moved_from.take_exclusion(&meta.file_hash)
            && let Some(folder) = source.folder_of(tx, &file.path, &mut caches.folder)?
        {
            queries::excluded_files::repoint_on(tx, exclusion_id, file_path_str, folder.id())?;
            log::info!("Kept-out file moved: {old_path} -> {file_path_str}");
            continue;
        }

        let Some(ids) = resolve_ids(tx, meta, source, file, &mut caches)? else {
            continue;
        };
        let row = queries::scan::NewTrackRow {
            file_path: file_path_str.to_owned(),
            file_name: file_name_of(&file.path),
            cue: None,
            meta,
            ids,
        };
        inserts.push(tx, row, &mut caches.names)?;
    }

    inserts.flush(tx, &mut caches.names)?;
    flush_artwork_backfill(tx, artwork_backfill)?;
    queries::excluded_files::restamp_on(tx, &kept_out_read)?;

    let inserted_track_ids = inserts.inserted_ids;
    Ok(IngestResult {
        inserted_count: u32::try_from(inserted_track_ids.len()).unwrap_or(u32::MAX),
        moved_count,
        updated_count,
        inserted_track_ids,
    })
}

/// The stored size and mtime of each scanned path that already has a whole-file row, a row with no
/// links stamped as never read, so a library folder absorbing an import's directory links it.
///
/// Chunked over `scanned_files` itself, so a chunk's binds drop at its end rather than a `String`
/// per scanned path living for the whole ingest.
fn load_existing(
    tx: &Transaction<'_>,
    scanned_files: &[ScannedFile],
) -> Result<HashMap<String, FileStamp>, AppError> {
    let mut existing_tracks = HashMap::with_capacity(scanned_files.len());
    for chunk in scanned_files.chunks(MAX_BINDS_PER_STATEMENT) {
        let path_list = placeholders(chunk.len());
        let mut sql = Sql::vouched(format!(
            "SELECT file_path, file_size, {LINKED_STAMP} FROM tracks \
             WHERE file_path IN ({path_list}) AND {WHOLE_FILE}"
        ));
        for file in chunk {
            sql.bind(file.path.to_string_lossy().into_owned());
        }
        let rows: Vec<(String, Option<i64>, Option<String>)> = sql.fetch_all(tx)?;
        for (path, file_size, date_modified) in rows {
            existing_tracks.insert(path, FileStamp { file_size, date_modified });
        }
    }
    Ok(existing_tracks)
}

/// The scanned paths a folder scan keeps out, asked inside the transaction that would insert them.
fn kept_out(
    tx: &Transaction<'_>,
    source: &IngestSource,
    scanned_files: &[ScannedFile],
) -> Result<HashSet<String>, AppError> {
    if !source.keeps_out_excluded() {
        return Ok(HashSet::new());
    }
    let paths: Vec<Cow<'_, str>> = scanned_files.iter().map(|f| f.path.to_string_lossy()).collect();
    let paths: Vec<&str> = paths.iter().map(AsRef::as_ref).collect();
    queries::excluded_files::among_on(tx, &paths)
}

/// The rows and kept-out files a new path may have moved from, by hash: those whose own path is
/// gone from disk or only respelled by a new path, one still there otherwise being a duplicate the
/// new file sits beside.
///
/// The stat runs with the writer transaction open: moving it ahead would change how a concurrent
/// delete races it.
fn resolve_move_candidates(
    tx: &Transaction<'_>,
    scanned_files: &[ScannedFile],
    existing_tracks: &HashMap<String, FileStamp>,
    pool: &ScanPool,
) -> Result<MoveCandidates, AppError> {
    let new_files: Vec<&ScannedFile> = scanned_files
        .iter()
        .filter(|f| !existing_tracks.contains_key(f.path.to_string_lossy().as_ref()))
        .collect();
    let new_path_hashes: Vec<&str> =
        new_files.iter().map(|f| f.metadata.file_hash.as_str()).collect();
    let candidates = MoveCandidates::read_on(tx, &new_path_hashes, &[])?;
    Ok(keep_gone(candidates, &new_files, pool))
}

/// The cue sheet images the `claimed` ones may have moved from, kept as
/// [`resolve_move_candidates`] keeps a row.
fn image_move_candidates(
    tx: &Transaction<'_>,
    claimed: &[ScannedFile],
    pool: &ScanPool,
) -> Result<MoveCandidates, AppError> {
    if claimed.is_empty() {
        return Ok(MoveCandidates::default());
    }
    let claimed: Vec<&ScannedFile> = claimed.iter().collect();
    let hashes: Vec<&str> = claimed.iter().map(|f| f.metadata.file_hash.as_str()).collect();
    let candidates = MoveCandidates::read_on(tx, &[], &hashes)?;
    Ok(keep_gone(candidates, &claimed, pool))
}

fn keep_gone(
    mut candidates: MoveCandidates,
    new_files: &[&ScannedFile],
    pool: &ScanPool,
) -> MoveCandidates {
    let mut still_present = batch_still_on_disk(candidates.old_paths(), pool);
    if queries::DISK_IGNORES_CASE {
        forget_respelled(&mut still_present, new_files);
    }
    candidates.retain_gone(&still_present);
    candidates
}

/// Drops the old paths a new file only respells: where the disk ignores case, two paths folding
/// alike are one file, so the new one can't be a copy sitting beside it.
fn forget_respelled(still_present: &mut HashSet<String>, new_files: &[&ScannedFile]) {
    let spelling_by_fold: HashMap<String, Cow<'_, str>> = new_files
        .iter()
        .map(|f| {
            let spelling = f.path.to_string_lossy();
            (spelling.to_lowercase(), spelling)
        })
        .collect();
    still_present.retain(|old_path| {
        spelling_by_fold.get(&old_path.to_lowercase()).is_none_or(|new_path| new_path == old_path)
    });
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

    fn push(
        &mut self,
        tx: &Transaction<'_>,
        row: queries::scan::NewTrackRow<'a>,
        names: &mut NameCache,
    ) -> Result<(), AppError> {
        self.pending.push(row);
        if self.pending.len() >= queries::scan::INSERT_CHUNK_ROWS {
            self.flush(tx, names)?;
        }
        Ok(())
    }

    fn flush(&mut self, tx: &Transaction<'_>, names: &mut NameCache) -> Result<(), AppError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let ids =
            queries::scan::insert_tracks_batch(tx, &self.pending, self.scan_timestamp, names)?;
        self.inserted_ids.extend(ids);
        self.pending.clear();
        Ok(())
    }
}

/// Below this the rayon overhead outweighs the stats, so the small batches incremental rescans
/// make walk sequentially.
const STAT_PAR_THRESHOLD: usize = 32;

/// The subset of `paths` still on disk under their own spelling, a batch past
/// [`STAT_PAR_THRESHOLD`] stat'ed on the scan pool.
///
/// **Under their own spelling** because a case-insensitive or normalizing filesystem answers
/// `exists()` for a spelling no entry carries, and a rename that changed only case or normalization
/// is a move. Only the file name is asked, the folder root above it being spelled as the user added
/// it, which leaves a directory renamed by case alone to [`forget_respelled`].
fn batch_still_on_disk(paths: Vec<String>, pool: &ScanPool) -> HashSet<String> {
    if paths.is_empty() {
        return HashSet::new();
    }
    if paths.len() < STAT_PAR_THRESHOLD {
        return listed_as_spelled(paths.into_iter().filter(|p| Path::new(p).exists()).collect());
    }
    let existing = pool.install(|| {
        paths.par_iter().filter(|p| Path::new(p).exists()).cloned().collect::<HashSet<String>>()
    });
    listed_as_spelled(existing)
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
fn flush_artwork_backfill(
    tx: &Transaction<'_>,
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
        let mut sql = Sql::vouched(format!(
            "WITH v(path, art) AS (VALUES {rows})
             UPDATE tracks
                SET artwork_path = (SELECT art FROM v WHERE v.path = tracks.file_path)
              WHERE file_path IN (SELECT path FROM v) AND {WHOLE_FILE}
                AND (artwork_path IS NULL OR artwork_path = '')"
        ));
        for (path, art) in chunk {
            sql.bind(path.as_str()).bind(art.as_str());
        }
        sql.execute(tx)?;
    }
    Ok(())
}

/// [`IngestSource::Import`]'s folder for `path`.
fn import_folder_of(
    tx: &Transaction<'_>,
    path: &Path,
    folder_cache: &mut HashMap<String, TrackFolder>,
) -> Result<Option<TrackFolder>, AppError> {
    let parent_dir = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    if parent_dir.is_empty() {
        log::warn!("Skipping file with no parent directory: {}", path.display());
        return Ok(None);
    }
    if let Some(&folder) = folder_cache.get(&parent_dir) {
        return Ok(Some(folder));
    }
    // A file inside a library folder is that folder's, so its scans pair the file with its row and
    // removing the folder takes the track along.
    let folder = match queries::scan::find_folder_for_path(tx, &path.to_string_lossy())? {
        Some(id) => TrackFolder::Library(id),
        None => TrackFolder::Outside(queries::folder::upsert_folder(tx, &parent_dir)?),
    };
    folder_cache.insert(parent_dir, folder);
    Ok(Some(folder))
}

/// A track's artist, album, genre and folder ids, or `None` where the file is skipped.
fn resolve_ids(
    tx: &Transaction<'_>,
    meta: &ExtractedMetadata,
    source: &IngestSource,
    file: &ScannedFile,
    caches: &mut ResolveCaches,
) -> Result<Option<queries::ResolvedIds>, AppError> {
    let folder_id = match source.folder_of(tx, &file.path, &mut caches.folder)? {
        None => return Ok(None),
        Some(TrackFolder::Outside(id)) => return Ok(Some(queries::ResolvedIds::outside(id))),
        Some(TrackFolder::Library(id)) => id,
    };

    // The **first** credited name, as `resolve_track_context` takes: a row keyed on the whole line
    // is an artist nobody recorded under.
    let artist_name = meta.artist.primary_name();
    let album_name = meta.album.as_deref().unwrap_or("");
    let genre_name = meta.genres.primary().unwrap_or("");

    let artist_id = caches.names.artist(tx, artist_name, UNKNOWN_ARTIST_ID)?;

    // The album groups by its album artist, so a per-track featured credit doesn't split it.
    let album_artist_name = queries::scan::album_artist_name_for(meta);
    let album_artist_id = if album_artist_name == artist_name {
        artist_id
    } else {
        caches.names.artist(tx, album_artist_name, UNKNOWN_ARTIST_ID)?
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
        )?;
        caches.album.entry(album_name.to_owned()).or_default().insert(album_artist_id, id);
        id
    };

    let genre_id = caches.names.genre(tx, genre_name)?;

    let links = Links { artist_id, album_id, genre_id };
    Ok(Some(queries::ResolvedIds { folder_id, links: Some(links) }))
}

#[cfg(test)]
#[path = "tests/ingest_tests.rs"]
mod tests;
