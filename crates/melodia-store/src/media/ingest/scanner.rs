use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use walkdir::{DirEntry, WalkDir};

use crate::media::ingest::metadata::extract_or_filename_row;
use melodia_core::entities::scan::{FileStamp, FileStampRef, ScannedFile};
use melodia_core::utils::audio_ext::{is_audio_extension, is_cue_sheet};
use melodia_core::utils::trash_dir::is_trash_dir;

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

/// The audio files and cue sheets under one folder, and what the walk couldn't vouch for.
pub struct MediaWalk {
    pub files: WalkedPaths,
    pub sheets: WalkedPaths,
    /// Paths the walk failed to read, a directory or now and then a single entry. Nothing under
    /// one is known to be gone, so a row there can't be taken for an orphan.
    pub unreadable: Vec<PathBuf>,
}

/// A walk's paths in a few large blocks, which glibc maps on their own, rather than one allocation
/// each: those, alive through the reconcile, can share an arena with the boot's long-lived ones and
/// leave them spread across the heap they grew. A path that isn't UTF-8 keeps its `PathBuf` intact.
#[derive(Default)]
pub struct WalkedPaths {
    text: String,
    entries: Vec<WalkedPath>,
    foreign: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
enum WalkedPath {
    Text { start: usize, end: usize },
    Foreign(usize),
}

impl WalkedPaths {
    fn push(&mut self, path: &Path) {
        let entry = if let Some(text) = path.to_str() {
            let start = self.text.len();
            self.text.push_str(text);
            WalkedPath::Text { start, end: self.text.len() }
        } else {
            self.foreign.push(path.to_path_buf());
            WalkedPath::Foreign(self.foreign.len() - 1)
        };
        self.entries.push(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn path_at(&self, index: usize) -> &Path {
        self.path(self.entries[index])
    }

    fn path(&self, entry: WalkedPath) -> &Path {
        match entry {
            WalkedPath::Text { start, end } => Path::new(&self.text[start..end]),
            WalkedPath::Foreign(slot) => &self.foreign[slot],
        }
    }

    /// The lossy string a row stores for the path, borrowed wherever the path is UTF-8.
    fn spelling(&self, entry: WalkedPath) -> Cow<'_, str> {
        match entry {
            WalkedPath::Text { start, end } => Cow::Borrowed(&self.text[start..end]),
            WalkedPath::Foreign(slot) => self.foreign[slot].to_string_lossy(),
        }
    }

    /// Orders the paths by [`Self::spelling`] on the current rayon pool.
    fn sort(&mut self) {
        let mut entries = std::mem::take(&mut self.entries);
        entries.par_sort_unstable_by(|&a, &b| self.spelling(a).cmp(&self.spelling(b)));
        self.entries = entries;
    }

    /// The indices of the paths spelled `path`, once sorted.
    fn spelled(&self, path: &str) -> Range<usize> {
        let start = self.entries.partition_point(|&entry| self.spelling(entry).as_ref() < path);
        let matching =
            self.entries[start..].partition_point(|&entry| self.spelling(entry).as_ref() == path);
        start..start + matching
    }

    /// The paths directly inside `dir`, once sorted. Every path starting with its spelling sits in
    /// one run of the sort, deeper ones and a sibling like `Album (2)` beside `Album` alike.
    fn in_dir<'a>(&'a self, dir: &'a Path) -> impl Iterator<Item = &'a Path> {
        let prefix = dir.to_string_lossy();
        let start =
            self.entries.partition_point(|&entry| self.spelling(entry).as_ref() < prefix.as_ref());
        self.entries[start..]
            .iter()
            .take_while(move |&&entry| self.spelling(entry).starts_with(prefix.as_ref()))
            .map(|&entry| self.path(entry))
            .filter(move |path| path.parent() == Some(dir))
    }
}

/// Walks `dir` for audio files and cue sheets, or answers `None` once `observer` cancels: a
/// partial list is one a caller could mistake for the whole tree.
pub fn collect_media_files(dir: &Path, observer: &dyn ScanObserver) -> Option<MediaWalk> {
    let mut files = WalkedPaths::default();
    let mut sheets = WalkedPaths::default();
    let mut unreadable = Vec::new();

    let walk = WalkDir::new(dir).follow_links(false).into_iter();
    for entry in walk.filter_entry(|entry| !is_trash_below_root(entry)) {
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
            files.push(path);
            observer.found(u32::try_from(files.len()).unwrap_or(u32::MAX));
        } else if is_cue_sheet(ext) {
            sheets.push(path);
        }
    }

    Some(MediaWalk { files, sheets, unreadable })
}

/// A root is walked whatever its name, as the user added it on purpose.
fn is_trash_below_root(entry: &DirEntry) -> bool {
    entry.depth() > 0 && entry.file_type().is_dir() && is_trash_dir(entry.path())
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

            if current.is_multiple_of(10) || current as usize == total {
                let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                observer.read(current, file_name);
            }

            // The incremental filter let only new or changed files through, so the full extract
            // is owed.
            match extract_or_filename_row(path, artwork_dir, cover_cache, false) {
                Ok(metadata) => Some(ScannedFile { path: path.clone(), metadata }),
                // An unreadable file; unparseable tags still make a filename row.
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

/// A folder's walk sorted by path, each file paired with the stamp its row or its exclusion holds,
/// if any: the audio files against their tracks, or the cue sheets against `cue_sheets`.
///
/// Rows arrive one at a time and find their files by binary search, so only an orphan's path is
/// copied, and the sort is on the lossy string a row stores so the two compare equal. Orphans are
/// the rows the **whole** walk didn't find, never those of the files left to parse, which would
/// read a no-change rescan's entire library as gone.
pub struct WalkIndex {
    files: WalkedPaths,
    stamps: Vec<Option<PackedStamp>>,
    /// Every stamp's mtime end to end, packed for the reason [`WalkedPaths`] packs its paths.
    stamp_text: String,
    unreadable: Vec<PathBuf>,
    orphans: Vec<String>,
}

/// A [`FileStamp`] whose mtime is a span of [`WalkIndex::stamp_text`].
#[derive(Clone, Copy)]
struct PackedStamp {
    file_size: Option<i64>,
    date_modified: Option<(usize, usize)>,
}

impl WalkIndex {
    /// Sorts `files` on the current rayon pool.
    pub fn new(mut files: WalkedPaths, unreadable: Vec<PathBuf>) -> Self {
        files.sort();
        let stamps = vec![None; files.len()];
        Self { files, stamps, stamp_text: String::new(), unreadable, orphans: Vec::new() }
    }

    /// How many files the walk found.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the walk found no file at all.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Whether the walk found `path`.
    pub fn contains(&self, path: &Path) -> bool {
        !self.files.spelled(&path.to_string_lossy()).is_empty()
    }

    /// The walked files directly inside `dir`.
    pub fn in_dir<'a>(&'a self, dir: &'a Path) -> impl Iterator<Item = &'a Path> {
        self.files.in_dir(dir)
    }

    /// Pairs a stored row with the files at its path, or keeps it as an orphan when there are
    /// none. A row under a path the walk couldn't read is spared: its file is as likely there as
    /// gone, and purging it takes the track's rating and play count with it.
    ///
    /// A sheet's rows share their image's path and stamp, so a file already stamped keeps it.
    pub fn add_row(&mut self, path: String, stamp: FileStamp) {
        let found = self.files.spelled(&path);
        if found.is_empty() {
            if !self.under_unreadable(&path) {
                self.orphans.push(path);
            }
            return;
        }
        if self.stamps[found.start].is_some() {
            return;
        }
        self.stamp_files(found, stamp);
    }

    /// Pairs a kept-out file's stamp with the files at its path, so one is read again only once it
    /// has changed. One the walk didn't find is [`Self::unwalked`]'s.
    pub fn add_kept_out(&mut self, path: &str, stamp: FileStamp) {
        let found = self.files.spelled(path);
        self.stamp_files(found, stamp);
    }

    /// The files `keep` admits that need reading: none stamped, or a size or mtime that has moved
    /// since. Runs on the current rayon pool, a `stat` per stamped file. Every launch with watching
    /// on re-scans each folder to find almost nothing, so this is the scan's common case.
    pub fn changed(&self, keep: impl Fn(&Path) -> bool + Sync) -> Vec<PathBuf> {
        (0..self.files.len())
            .into_par_iter()
            .filter_map(|index| {
                let file = self.files.path_at(index);
                let read_again = keep(file)
                    && !self.stamps[index]
                        .is_some_and(|stamp| file_is_current(file, self.unpack(stamp)));
                read_again.then(|| file.to_path_buf())
            })
            .collect()
    }

    /// The paths of the rows the walk didn't find, each once however many rows it held.
    pub fn take_orphans(&mut self) -> Vec<String> {
        let mut orphans = std::mem::take(&mut self.orphans);
        orphans.sort_unstable();
        orphans.dedup();
        orphans
    }

    /// The kept-out paths among `paths` the walk didn't find, sparing those under a path it
    /// couldn't read as [`Self::add_row`] spares a row.
    pub fn unwalked(&self, paths: impl IntoIterator<Item = String>) -> Vec<String> {
        paths
            .into_iter()
            .filter(|path| self.files.spelled(path).is_empty() && !self.under_unreadable(path))
            .collect()
    }

    fn stamp_files(&mut self, found: Range<usize>, stamp: FileStamp) {
        if found.is_empty() {
            return;
        }
        let date_modified = stamp.date_modified.map(|text| {
            let start = self.stamp_text.len();
            self.stamp_text.push_str(&text);
            (start, self.stamp_text.len())
        });
        // More than one only for paths that aren't valid UTF-8 and read back alike.
        self.stamps[found].fill(Some(PackedStamp { file_size: stamp.file_size, date_modified }));
    }

    fn unpack(&self, stamp: PackedStamp) -> FileStampRef<'_> {
        FileStampRef {
            file_size: stamp.file_size,
            date_modified: stamp.date_modified.map(|(start, end)| &self.stamp_text[start..end]),
        }
    }

    fn under_unreadable(&self, path: &str) -> bool {
        self.unreadable.iter().any(|dir| Path::new(path).starts_with(dir))
    }
}

/// Whether `path` still matches `stamp`, so the parse can skip it. A heuristic rather than a byte
/// identity, but any normal write bumps the mtime; a file that can't be stat'ed isn't current.
pub fn file_is_current(path: &Path, stamp: FileStampRef<'_>) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let on_disk_size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
    let on_disk_mtime = crate::media::ingest::metadata::date_modified_from_metadata(&meta);
    stamp.matches(on_disk_size, on_disk_mtime.as_deref())
}

#[cfg(test)]
#[path = "tests/scanner_tests.rs"]
mod tests;
