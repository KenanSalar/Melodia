use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::media::ingest::metadata::extract_or_filename_row;
use melodia_core::entities::scan::{ExistingTrackSummary, ScannedFile};
use melodia_core::utils::audio_ext::is_audio_extension;

/// What a library walk and parse report while they run, and how they learn to stop.
///
/// Both run on blocking threads nothing outside can abort, so a scan stops only where it asks
/// [`is_cancelled`](Self::is_cancelled).
pub trait ScanObserver: Sync {
    fn is_cancelled(&self) -> bool;

    /// Receives the running count of audio files the walk has found, once per file.
    fn found(&self, _count: u32) {}

    /// Receives how many files the parse has read, every few files and on the last.
    fn read(&self, _done: u32, _file_name: &str) {}
}

/// The observer for a parse nobody watches or stops, such as a file-drop import.
pub struct Unobserved;

impl ScanObserver for Unobserved {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// The audio files under one folder, and what the walk couldn't vouch for.
pub struct MediaWalk {
    pub files: Vec<PathBuf>,
    /// Paths the walk failed to read, a directory or now and then a single entry. Nothing under
    /// one is known to be gone, so a row there can't be taken for an orphan.
    pub unreadable: Vec<PathBuf>,
}

/// Walks `dir` for audio files, or answers `None` once `observer` cancels: a partial list is
/// one a caller could mistake for the whole tree.
pub fn collect_media_files(dir: &Path, observer: &dyn ScanObserver) -> Option<MediaWalk> {
    let mut files = Vec::with_capacity(256);
    let mut unreadable = Vec::new();

    for entry in WalkDir::new(dir).follow_links(false) {
        if observer.is_cancelled() {
            return None;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log::warn!(
                    "Skipping an unreadable path under {}: {}",
                    dir.display(),
                    melodia_core::error::describe(&e)
                );
                // An error naming no path vouches for nothing below the root.
                unreadable.push(e.path().unwrap_or(dir).to_path_buf());
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        if is_audio_extension(ext) {
            files.push(path.to_path_buf());
            observer.found(u32::try_from(files.len()).unwrap_or(u32::MAX));
        }
    }

    Some(MediaWalk { files, unreadable })
}

/// Parses `files` on the current rayon pool. Once `observer` cancels, the files not yet started
/// are skipped, so what comes back is whatever had been read by then.
pub fn scan_files_parallel(
    files: &[PathBuf],
    artwork_dir: &Path,
    cover_cache: &melodia_artwork::media::image::artwork::CoverCache,
    observer: &dyn ScanObserver,
) -> Vec<ScannedFile> {
    let total = files.len();
    let scanned = std::sync::atomic::AtomicU32::new(0);

    files
        .par_iter()
        .filter_map(|path| {
            if observer.is_cancelled() {
                return None;
            }
            let current = scanned.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;

            // Report progress every 10 files
            if current.is_multiple_of(10) || current as usize == total {
                let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                observer.read(current, file_name);
            }

            // Every file reaching this point was selected by the caller's
            // incremental filter (`track_is_current`) as new or changed, so
            // a full extract — embedded artwork included — is always
            // warranted. Unchanged files never get here.
            match extract_or_filename_row(path, artwork_dir, cover_cache, false) {
                Ok(metadata) => Some(ScannedFile { path: path.clone(), metadata }),
                // Only an unreadable file gets this far now; unparseable tags come back
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
        .collect()
}

/// A folder's walk sorted by path, each file paired with the row it already has, if any.
///
/// What the incremental filter and the orphan purge need, without a second copy of every path:
/// rows arrive one at a time and find their files by binary search, and only an orphan's path is
/// kept. Sorted by the same lossy string a row stores, so a file and its row compare equal.
///
/// Orphans are the rows the **whole** walk didn't find, never the rows of the files left to
/// parse: the filter leaves unchanged files out of those, and a no-change rescan would read the
/// entire library as gone.
pub struct WalkIndex {
    files: Vec<PathBuf>,
    rows: Vec<Option<ExistingTrackSummary>>,
    unreadable: Vec<PathBuf>,
    orphans: Vec<String>,
}

impl WalkIndex {
    /// Sorts `walk` on the current rayon pool.
    pub fn new(walk: MediaWalk) -> Self {
        let MediaWalk { mut files, unreadable } = walk;
        files.par_sort_unstable_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
        let rows = vec![None; files.len()];
        Self { files, rows, unreadable, orphans: Vec::new() }
    }

    /// How many audio files the walk found.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the walk found no audio file at all.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Pairs a stored row with the files at its path, or keeps it as an orphan when there are
    /// none. A row under a path the walk couldn't read is spared: its file is as likely there as
    /// gone, and purging it takes the track's rating and play count with it.
    pub fn add_row(&mut self, path: String, row: ExistingTrackSummary) {
        let start =
            self.files.partition_point(|file| file.to_string_lossy().as_ref() < path.as_str());
        let matching = self.files[start..]
            .partition_point(|file| file.to_string_lossy().as_ref() == path.as_str());
        if matching == 0 {
            if !self.unreadable.iter().any(|dir| Path::new(&path).starts_with(dir)) {
                self.orphans.push(path);
            }
            return;
        }
        // More than one only for paths that aren't valid UTF-8 and read back alike.
        if let Some((last, others)) = self.rows[start..start + matching].split_last_mut() {
            for slot in others {
                *slot = Some(row.clone());
            }
            *last = Some(row);
        }
    }

    /// The files `keep` admits that need reading: no row yet, or a size or mtime that has moved
    /// since. Runs on the current rayon pool, a `stat` per file that has a row.
    pub fn changed(&self, keep: impl Fn(&Path) -> bool + Sync) -> Vec<PathBuf> {
        self.files
            .par_iter()
            .zip(&self.rows)
            .filter(|(file, row)| {
                keep(file) && !row.as_ref().is_some_and(|row| track_is_current(file, row))
            })
            .map(|(file, _)| file.clone())
            .collect()
    }

    /// The rows the walk didn't find.
    pub fn into_orphans(self) -> Vec<String> {
        self.orphans
    }
}

/// True when the on-disk file matches its existing DB row and needs no
/// re-scan: the file's size **and** mtime both match `row`. Size + mtime
/// unchanged is a heuristic — not a byte-identity guarantee — but it reliably
/// catches tag and artwork edits, since any normal write bumps the mtime.
/// Callers use this as an incremental-scan filter: a `true` result means Lofty
/// can be skipped entirely for the file.
///
/// A `false` result — a changed size or mtime, or a file that can't be stat'ed
/// — means the file must be (re)parsed in full so its metadata and cover are
/// brought up to date.
pub fn track_is_current(path: &Path, row: &ExistingTrackSummary) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let on_disk_size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    if row.file_size != Some(on_disk_size) {
        return false;
    }
    // Derive the mtime string from the `meta` already in hand — no second
    // `stat`. Goes through the shared formatter so it can't drift from the
    // format `extract_date_modified` stored in `date_modified`.
    let on_disk_mtime = crate::media::ingest::metadata::date_modified_from_metadata(&meta);
    on_disk_mtime.is_some() && on_disk_mtime.as_deref() == row.date_modified.as_deref()
}

#[cfg(test)]
#[path = "tests/scanner_tests.rs"]
mod tests;
