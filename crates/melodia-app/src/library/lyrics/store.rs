//! Sheets the directory answered with, kept on disk so a replay costs no traffic.
//!
//! Nothing else names these files, so the filesystem is the whole index and retention asks only
//! how old a file is.
//!
//! **The name hashes the track's path, not its contents**, though it wears the artwork store's 16
//! hex characters: a tag edit moves the content hash, and a sheet keyed on it would be orphaned by
//! the user fixing a typo in the title.

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

/// How long "nobody has this" stands. The directory grows, so a miss is about today rather than
/// the recording: long enough that a library on repeat isn't re-asking, short enough that a sheet
/// contributed last month is found.
const MISS_STANDS_FOR: Duration = Duration::from_hours(30 * 24);

/// What the store has for a track, or `None`. A file that won't read is a miss, the caller's next
/// move being the same either way.
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

/// What came back, as the store keeps it. A sheet crosses as the directory's text, so re-reading
/// it later can't differ from reading it now.
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

fn still_stands(marker: &Path) -> bool {
    let Ok(modified) = fs::metadata(marker).and_then(|meta| meta.modified()) else {
        return false;
    };
    // A marker from the future is a clock that moved. Read as expired it costs one request; read
    // as fresh it would answer until the clock caught up.
    modified.elapsed().is_ok_and(|age| age < MISS_STANDS_FOR)
}

/// Drop every answer the store holds for one track, which is what makes a re-ask possible. Every
/// name is attempted, the first error coming back: stopping at the sheet would leave the absent
/// marker answering for the month.
pub(super) fn forget(dir: &Path, track_path: &str) -> Result<(), AppError> {
    let stem = key(track_path);
    let mut first_error = None;
    for extension in [SHEET_EXT, INSTRUMENTAL_EXT, ABSENT_EXT] {
        match fs::remove_file(entry(dir, &stem, extension)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => first_error = first_error.or(Some(AppError::io_source(e))),
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// The stored sheet's text, verbatim. The markers answer a different question.
pub(super) fn read_text(dir: &Path, track_path: &str) -> Option<String> {
    fs::read_to_string(entry(dir, &key(track_path), SHEET_EXT))
        .ok()
        .filter(|text| !text.trim().is_empty())
}

fn entry(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    dir.join(format!("{stem}.{extension}"))
}

fn key(track_path: &str) -> String {
    blake3::hash(track_path.as_bytes()).to_hex()[..HASH_HEX_LEN].to_owned()
}

/// Bytes of sheets the store may hold before the oldest are retired, thousands of tracks' worth.
/// Markers neither count nor go: empty, evicting one frees nothing and costs the request it saved.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Retire expired misses, then the oldest sheets until the store fits. **Only names this module
/// writes are touched**, the artwork sweep's rule.
pub(super) fn prune(dir: &Path) -> Result<u32, AppError> {
    let mut sheets: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let mut expired = 0;
    let mut bytes = 0;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(extension) = stored_extension(&path) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };

        if extension == ABSENT_EXT {
            expired += u32::from(expire_stale_marker(&path));
            continue;
        }
        // Sheets only: past the budget an empty instrumental marker in this list would go for no
        // bytes, a permanent answer traded for nothing.
        if extension != SHEET_EXT {
            continue;
        }

        bytes += meta.len();
        sheets.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), path));
    }

    Ok(expired + evict_oldest_over_budget(sheets, bytes))
}

/// Delete a miss marker whose window has closed, saying whether it went. `read` already ignores a
/// stale one, so this only keeps the folder from holding one per track.
fn expire_stale_marker(path: &Path) -> bool {
    !still_stands(path) && fs::remove_file(path).is_ok()
}

/// Delete sheets oldest-first until the store is inside [`MAX_BYTES`], answering how many went.
/// Oldest by write time, nothing here being touched on a hit.
fn evict_oldest_over_budget(mut sheets: Vec<(SystemTime, u64, PathBuf)>, mut bytes: u64) -> u32 {
    if bytes <= MAX_BYTES {
        return 0;
    }
    sheets.sort_by_key(|(modified, _, _)| *modified);

    let mut evicted = 0;
    for (_, len, path) in sheets {
        if bytes <= MAX_BYTES {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            bytes = bytes.saturating_sub(len);
            evicted += 1;
        }
    }
    evicted
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
