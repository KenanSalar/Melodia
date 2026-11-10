//! Files opened from outside: a drop, "Open with", the command line, a kept-out file brought back.
//!
//! Each opened path stands for its tracks in play order: a file for every row it has, so an image a
//! cue sheet splits for one per track, and a sheet for the files it names. A file new to the
//! library is read in with the sheets around it settled first (`cue::settle_dirs`), so an image
//! opened beside its sheet is split rather than taken whole.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::audio_ext::is_library_extension;
use melodia_store::database::queries::ingest::ScanChunk;
use melodia_store::database::queries::scan::{self, SheetsAround};
use melodia_store::database::queries::track::FileTracks;
use melodia_store::database::{DbPool, queries};
use melodia_store::media::cue;
use melodia_store::media::ingest::scan_pool::ScanPool;
use melodia_store::media::ingest::scanner::{Unobserved, scan_files_parallel};
use melodia_store::media::ingest::watcher::is_sheet_file;

/// `summaries` costs a second query, so only [`import_and_summarize`] fills it.
#[derive(Clone)]
pub struct ImportFilesResult {
    /// Each opened path's tracks, in the order the paths came. A track opened twice, by its own
    /// path and by its sheet, stays where it came first.
    pub opened: Vec<Vec<i64>>,
    pub imported_count: u32,
    pub failed_paths: Vec<String>,
    /// In play order.
    pub summaries: Vec<Arc<TrackSummary>>,
}

/// Imports audio files and cue sheets, answering the tracks of every valid one, new or already in
/// the library. Takes the three pieces it reaches rather than `&AppState`, so a drop can be
/// replayed against a `test_pool`.
pub(crate) async fn import_files(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    let (valid_paths, mut failed_paths) = sort_out(file_paths);
    let mut items: Vec<(String, Vec<String>)> = Vec::with_capacity(valid_paths.len());
    for (path, files) in valid_paths.iter().zip(files_opened_by(&valid_paths).await?) {
        if files.is_empty() {
            failed_paths.push(path.clone());
        } else {
            items.push((path.clone(), files));
        }
    }
    if items.is_empty() {
        return Ok(ImportFilesResult {
            opened: Vec::new(),
            imported_count: 0,
            failed_paths,
            summaries: Vec::new(),
        });
    }

    // A file given twice, or by itself and by its sheet, is one track: left in, the batch insert
    // meets its own row and the whole import fails on `tracks.file_path`.
    let mut seen = HashSet::new();
    let files: Vec<String> =
        items.iter().flat_map(|(_, files)| files).filter(|f| seen.insert(*f)).cloned().collect();
    let sheets: Vec<PathBuf> =
        items.iter().map(|(path, _)| PathBuf::from(path)).filter(|p| is_sheet_file(p)).collect();

    let mut rows = queries::track::get_track_ids_by_paths(db, &files).await?;
    let new_audio: Vec<PathBuf> =
        files.iter().filter(|file| !rows.contains(file)).map(PathBuf::from).collect();
    let dirs = parent_dirs(&new_audio, &sheets);
    let around = scan::sheets_around(db, dirs, lossy(&new_audio)).await?;
    let unrecorded: Vec<PathBuf> = sheets
        .into_iter()
        .filter(|sheet| !around.sheets.iter().any(|recorded| Path::new(recorded) == sheet))
        .collect();

    let mut imported_count = 0;
    if !new_audio.is_empty() || !unrecorded.is_empty() {
        let opened: HashSet<PathBuf> = files.iter().map(PathBuf::from).collect();
        let reading = Reading { new_audio, unrecorded, around, opened };
        let (chunk, pool) = read_in(reading, artwork_dir, cover_cache).await?;
        let scan_timestamp = melodia_core::utils::now_rfc3339();
        let result = queries::ingest::commit_import(db, chunk, scan_timestamp, pool).await?;
        imported_count = result.inserted_count;
        rows = queries::track::get_track_ids_by_paths(db, &files).await?;
    }

    Ok(ImportFilesResult {
        opened: tracks_opened(&items, &rows),
        imported_count,
        failed_paths,
        summaries: Vec::new(),
    })
}

/// The paths Melodia takes, canonical and each once, and those it refuses.
fn sort_out(file_paths: &[String]) -> (Vec<String>, Vec<String>) {
    let mut valid: Vec<String> = Vec::with_capacity(file_paths.len());
    let mut failed: Vec<String> = Vec::new();
    for file_path in file_paths {
        let path = PathBuf::from(file_path);
        let takes = path.extension().and_then(|e| e.to_str()).is_some_and(is_library_extension);
        match melodia_core::utils::canonicalize_path(&path) {
            Ok(canonical) if takes => valid.push(canonical.to_string_lossy().into_owned()),
            _ => failed.push(file_path.clone()),
        }
    }
    let mut seen = HashSet::with_capacity(valid.len());
    valid.retain(|path| seen.insert(path.clone()));
    (valid, failed)
}

/// The files each of `paths` opens, in order: the file itself, or the audio files a cue sheet
/// names. A sheet that names none opens nothing, and says why in the log.
pub(crate) async fn files_opened_by(paths: &[String]) -> Result<Vec<Vec<String>>, AppError> {
    let sheets: Vec<PathBuf> =
        paths.iter().map(PathBuf::from).filter(|path| is_sheet_file(path)).collect();
    let mut named = if sheets.is_empty() {
        Vec::new()
    } else {
        tokio::task::spawn_blocking(move || cue::named_files(&sheets))
            .await
            .map_err(|e| AppError::scanner("Cue sheet read task failed", e))?
    }
    .into_iter();

    let opened = paths.iter().map(|path| {
        if !is_sheet_file(Path::new(path)) {
            return vec![path.clone()];
        }
        match named.next() {
            Some(Ok(files)) => lossy(&files),
            Some(Err(e)) => {
                log::warn!("Opening {path}: {}", describe(&e));
                Vec::new()
            }
            None => Vec::new(),
        }
    });
    Ok(opened.collect())
}

/// What an import reads in beyond the rows the library holds.
struct Reading {
    new_audio: Vec<PathBuf>,
    /// Opened sheets the library holds no stamp for.
    unrecorded: Vec<PathBuf>,
    around: SheetsAround,
    /// Every file the paths opened, which bounds what a sheet may claim or give back here.
    opened: HashSet<PathBuf>,
}

/// Settles the sheets around what is new and reads it, each image a sheet claims or gives back
/// included.
async fn read_in(
    reading: Reading,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
) -> Result<(ScanChunk, ScanPool), AppError> {
    let artwork_dir = artwork_dir.to_path_buf();
    let cover_cache = cover_cache.clone();
    tokio::task::spawn_blocking(move || {
        let Reading { mut new_audio, unrecorded, around, opened } = reading;
        let written = new_audio.iter().chain(&unrecorded).cloned().collect();
        let plan = cue::settle_dirs(&around, written, |image| opened.contains(image));
        plan.take_out_of(&mut new_audio);
        let cue = plan.into_chunk();
        new_audio.extend(cue.images());

        let pool = ScanPool::for_files(new_audio.len());
        let files = pool
            .install(|| scan_files_parallel(&new_audio, &artwork_dir, &cover_cache, &Unobserved));
        (ScanChunk { files, cue }, pool)
    })
    .await
    .map_err(|e| AppError::scanner("Scan task failed", e))
}

/// Each opened item's tracks, a track met before left out where it repeats.
fn tracks_opened(items: &[(String, Vec<String>)], rows: &FileTracks) -> Vec<Vec<i64>> {
    let mut seen = HashSet::new();
    items
        .iter()
        .map(|(_, files)| {
            files.iter().flat_map(|file| rows.ids(file)).filter(|id| seen.insert(*id)).collect()
        })
        .filter(|tracks: &Vec<i64>| !tracks.is_empty())
        .collect()
}

/// The directories holding `files` and `sheets`, each once.
fn parent_dirs(files: &[PathBuf], sheets: &[PathBuf]) -> Vec<String> {
    let dirs: BTreeSet<&Path> = files.iter().chain(sheets).filter_map(|p| p.parent()).collect();
    dirs.into_iter().map(|dir| dir.to_string_lossy().into_owned()).collect()
}

fn lossy(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|path| path.to_string_lossy().into_owned()).collect()
}

/// [`import_files`] plus every imported track's `TrackSummary`, in one query, in play order.
pub(crate) async fn import_and_summarize(
    db: &DbPool,
    artwork_dir: &Path,
    cover_cache: &CoverCache,
    file_paths: &[String],
) -> Result<ImportFilesResult, AppError> {
    let mut result = import_files(db, artwork_dir, cover_cache, file_paths).await?;
    let ids = result.opened.concat();
    if ids.is_empty() {
        return Ok(result);
    }
    let by_id: HashMap<i64, Arc<TrackSummary>> =
        queries::track::get_track_summaries_by_ids(db, ids)
            .await?
            .into_iter()
            .map(|summary| (summary.id, Arc::new(summary)))
            .collect();
    let opened = result
        .opened
        .iter()
        .map(|tracks| tracks.iter().filter_map(|id| by_id.get(id).cloned()).collect::<Vec<_>>());
    result.summaries = in_play_order(opened.collect());
    Ok(result)
}

/// The opened items by title through `natord` ("Track 2" ahead of "Track 10"), each keeping its
/// own order, so a sheet's tracks play in sheet order behind its first. A drop arrives in visual
/// selection order on KDE and GNOME, and the sort is stable, so equal titles keep that order.
fn in_play_order(mut opened: Vec<Vec<Arc<TrackSummary>>>) -> Vec<Arc<TrackSummary>> {
    opened.sort_by(|a, b| natord::compare(head_title(a), head_title(b)));
    opened.concat()
}

fn head_title(tracks: &[Arc<TrackSummary>]) -> &str {
    tracks.first().map_or("", |track| track.title.as_str())
}

#[cfg(test)]
#[path = "tests/import_tests.rs"]
mod tests;
