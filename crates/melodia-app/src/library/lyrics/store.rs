//! Sheets the directory answered with, kept on disk so a replay costs no traffic.
//!
//! **Not the artwork store's shape, and the difference is what it does not need.** Nothing else in
//! the tree names these files, so there is no reference set to sweep against and no grace window
//! to get right: the filesystem is the whole index, and the only question retention asks is how
//! old a file is.
//!
//! **The name hashes the track's path, not its contents.** The artwork store hashes contents on
//! purpose, and this deliberately does not: a tag edit rewrites the file and moves its content
//! hash, and a sheet keyed that way would be orphaned by the user correcting a typo in the title.
//! The two schemes wear the same 16 hex characters and mean different things, which is why this
//! says so.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::lrc;
use melodia_core::entities::lyrics::{LyricsOutcome, LyricsSource};
use melodia_core::error::AppError;
use melodia_core::utils::atomic_file;

/// Half a BLAKE3, as the artwork store takes. Long enough that a collision needs more tracks than
/// anyone has, short enough to read in a directory listing.
const HASH_HEX_LEN: usize = 16;

/// A fetched sheet, verbatim as the directory sent it.
const SHEET_EXT: &str = "lrc";
/// A recording the directory says has no words. Permanent: the answer will not change.
const INSTRUMENTAL_EXT: &str = "instrumental";
/// A track nobody had a sheet for. Expires, unlike the other two.
const ABSENT_EXT: &str = "none";

/// How long "nobody has this" stands before it is worth asking again.
///
/// The directory is a volunteer corpus that grows, so a miss is a statement about today rather
/// than about the recording. Long enough that a library on repeat is not re-asking, short enough
/// that a sheet contributed last month is found this month.
const MISS_STANDS_FOR: Duration = Duration::from_hours(30 * 24);

/// What the store has for a track, or `None` where it has nothing usable.
///
/// **A file that will not read is a miss rather than an error**, and that is the honest semantics
/// rather than a swallowed failure: this is a cache, every answer it gives is an optimisation, and
/// the caller's next move for "unreadable" and for "absent" is the same one.
pub(super) fn read(dir: &Path, track_path: &str) -> Option<LyricsOutcome> {
    let stem = key(track_path);

    if let Ok(text) = fs::read_to_string(entry(dir, &stem, SHEET_EXT))
        && let Some(lyrics) = lrc::parse(&text, LyricsSource::Online)
    {
        return Some(LyricsOutcome::Sheet(lyrics));
    }
    if entry(dir, &stem, INSTRUMENTAL_EXT).is_file() {
        return Some(LyricsOutcome::Instrumental);
    }
    if still_stands(&entry(dir, &stem, ABSENT_EXT)) {
        return Some(LyricsOutcome::Absent);
    }
    None
}

/// What came back, in the form the store keeps it.
///
/// The sheet crosses as **text** rather than as a parsed [`Lyrics`]: what goes on disk is what the
/// directory sent, so re-reading it later cannot differ from reading it now, and a sheet written
/// back through a serializer would quietly lose whatever this parser drops.
#[derive(Clone, Copy)]
pub(super) enum Fetched<'a> {
    Sheet(&'a str),
    Instrumental,
    Nothing,
}

/// Records an answer, so the same track never costs a second request.
pub(super) fn write(dir: &Path, track_path: &str, fetched: Fetched<'_>) -> Result<(), AppError> {
    let stem = key(track_path);
    match fetched {
        Fetched::Sheet(text) => atomic_file::write_text_sync(&entry(dir, &stem, SHEET_EXT), text),
        Fetched::Instrumental => {
            atomic_file::write_text_sync(&entry(dir, &stem, INSTRUMENTAL_EXT), "")
        }
        Fetched::Nothing => atomic_file::write_text_sync(&entry(dir, &stem, ABSENT_EXT), ""),
    }
}

/// Whether a miss marker is young enough to still answer for.
fn still_stands(marker: &Path) -> bool {
    let Ok(modified) = fs::metadata(marker).and_then(|meta| meta.modified()) else {
        return false;
    };
    // A marker stamped in the future is a clock that moved, not a miss from tomorrow; treating it
    // as expired costs one request and treating it as fresh could cost every request forever.
    modified.elapsed().is_ok_and(|age| age < MISS_STANDS_FOR)
}

fn entry(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    dir.join(format!("{stem}.{extension}"))
}

fn key(track_path: &str) -> String {
    blake3::hash(track_path.as_bytes()).to_hex()[..HASH_HEX_LEN].to_owned()
}

/// Total bytes the store may hold before the oldest sheets are retired.
///
/// A sheet is a couple of kilobytes, so this is thousands of tracks: past that it is a cache of
/// songs nobody has played in a long time, and the oldest are the ones least likely to be wanted.
/// Only `.lrc` files can reach it, the two markers being empty.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Retire what the store no longer needs: expired misses first, then the oldest sheets until it
/// fits.
///
/// **Only names this module writes are touched**, which is the artwork sweep's rule and it applies
/// here for the same reason: this directory is under the user's data root, and a sweep that
/// deleted whatever it found would be a sweep that deleted whatever someone else put there.
pub(super) fn prune(dir: &Path) -> Result<u32, AppError> {
    let mut sheets: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let mut retired = 0;
    let mut total = 0;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(extension) = stored_extension(&path) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };

        if extension == ABSENT_EXT && !still_stands(&path) {
            // Deleting it *is* the retry: `read` already treats a stale marker as no answer, so
            // this only stops the directory keeping one per track forever.
            if fs::remove_file(&path).is_ok() {
                retired += 1;
            }
            continue;
        }

        total += meta.len();
        sheets.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), path));
    }

    if total <= MAX_BYTES {
        return Ok(retired);
    }

    // Oldest first, which for a cache of answers is least-recently-written rather than
    // least-recently-read: nothing here is touched on a hit, so a write time is all there is.
    sheets.sort_by_key(|(modified, _, _)| *modified);
    for (_, len, path) in sheets {
        if total <= MAX_BYTES {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
            retired += 1;
        }
    }
    Ok(retired)
}

/// The extension, if this is a name the store wrote.
fn stored_extension(path: &Path) -> Option<&str> {
    let stem = path.file_stem()?.to_str()?;
    let extension = path.extension()?.to_str()?;
    let named_here = stem.len() == HASH_HEX_LEN && stem.bytes().all(|b| b.is_ascii_hexdigit());
    let known = [SHEET_EXT, INSTRUMENTAL_EXT, ABSENT_EXT].contains(&extension);
    (named_here && known).then_some(extension)
}

#[cfg(test)]
#[path = "tests/store_tests.rs"]
mod tests;
