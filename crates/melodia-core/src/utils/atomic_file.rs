//! Whole-file read and atomic whole-file write: the small JSON and text files the app keeps beside
//! its database, and the tag rewrite of a track in the user's own library.
//!
//! The reads are plain and unsynchronised, and they are safe because the writes are not: every
//! write lands through a temp file in the same directory and a rename, so a reader sees either
//! the previous file entire or the new one entire, and a crash mid-write leaves the previous one
//! intact.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{AppError, AppResult};

/// Read JSON from `path`, falling back to `T::default()` on a missing file or a parse error. The
/// sync variant, for startup before the runtime exists.
pub fn load_json_or_default_sync<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!("Failed to parse {}, using defaults: {e}", path.display());
        T::default()
    }))
}

/// [`load_json_or_default_sync`]'s async twin.
pub async fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok(T::default());
    };
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!("Failed to parse {}, using defaults: {e}", path.display());
        T::default()
    }))
}

/// Write `value` as pretty JSON through a temp file in the same directory, renaming on success.
/// Nothing allocates the whole payload as a `String` first.
pub fn write_json_sync<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    write_with_sync(path, |writer| {
        serde_json::to_writer_pretty(writer, value).map_err(AppError::io_source)
    })
}

/// [`write_json_sync`]'s plain-text sibling. Bytes go out verbatim — the caller
/// owns line endings and the trailing newline.
pub fn write_text_sync(path: &Path, text: &str) -> AppResult<()> {
    write_with_sync(path, |writer| Ok(writer.write_all(text.as_bytes())?))
}

/// [`write_text_sync`] on the blocking pool, for a caller that must not block the thread it
/// awaits on.
pub async fn write_text(path: PathBuf, text: String) -> AppResult<()> {
    tokio::task::spawn_blocking(move || write_text_sync(&path, &text))
        .await
        .map_err(AppError::io_source)?
}

/// Hands `write` the temp file the other writers go through, renaming it over `path` only once
/// `write` succeeds. For a payload streamed out rather than held whole, such as a playlist archive.
///
/// The writer seeks as well as writes, which is what a zip needs to go back and fill in each
/// entry's header.
pub fn write_with_sync(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<&mut File>) -> AppResult<()>,
) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        write(&mut writer)?;
        writer.flush()?;
    }
    rename_over(tmp.into_temp_path(), path)
}

/// Hands `rewrite` a copy of `path` to work on, renaming it over the original only once `rewrite`
/// succeeds.
///
/// [`write_with_sync`]'s shape for a library that insists on opening the path itself, and it is
/// here for a second reason on top of the crash safety the others buy. **A rewrite in place moves
/// every byte after the edit under whoever already has the file open**, and a deck playing that
/// track keeps reading at offsets that now land somewhere else: an edit that grows a tag by a
/// megabyte does not sound like an edit, it sounds like the track breaking. Renaming leaves that
/// reader on the file it opened, whole to the end, and the next open gets the new one.
///
/// Taking the name off a file someone is reading is where Windows asks for more than a rename;
/// [`rename_over`] is what pays it.
///
/// Costs one copy of the file, which is why it is for whole-file rewrites: those pay it anyway,
/// and a write permission on the *directory* that an in-place save did not need. No fallback to
/// one: a second write path would be the one nobody exercises, and it is the path that reintroduces
/// the fault above.
pub fn rewrite_with_sync(
    path: &Path,
    rewrite: impl FnOnce(&Path) -> AppResult<()>,
) -> AppResult<()> {
    // Resolved first, because a rename replaces the *link* where a write in place followed it: a
    // symlinked track would otherwise become a regular file holding the edit, with the file the
    // user actually keeps left untouched.
    let path = &std::fs::canonicalize(path)?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    // Closed rather than held open: `rewrite` opens the path for itself, and the temp keeps no
    // extension, so a watcher over a music folder sees a file it does not scan.
    let tmp = tempfile::NamedTempFile::new_in(dir)?.into_temp_path();
    std::fs::copy(path, &tmp)?;
    rewrite(&tmp)?;
    rename_over(tmp, path)
}

/// Rename `tmp` over `path`, taking the temp with it if the rename fails.
///
/// Not `TempPath::persist`, which is a bare `MoveFileExW` on Windows: that call cannot take a name
/// another handle holds open, whatever share mode that handle was opened with, and a deck playing
/// the track being edited is exactly such a handle. `std::fs::rename` retries the same replace
/// through `FileRenameInfoEx` with POSIX semantics, the one form that unlinks a name out from
/// under a live reader. Unix never had the split, `persist` being `fs::rename` there.
///
/// A handle that denies delete sharing outright, an indexer or a scanner, still refuses both, and
/// still fails the write rather than corrupting it.
///
/// [`TempPath::keep`] first, for the half of `persist` that is not the move: the temp is created
/// `FILE_ATTRIBUTE_TEMPORARY` and both callers carry that onto the file they leave behind, so
/// without it every file written here is one the cache manager has been told not to flush.
///
/// [`TempPath::keep`]: tempfile::TempPath::keep
fn rename_over(tmp: tempfile::TempPath, path: &Path) -> AppResult<()> {
    let tmp = tmp.keep().map_err(|e| AppError::Io(e.error))?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        // `keep` disarmed the cleanup, and there is nothing to report past the rename's own error.
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/atomic_file_tests.rs"]
mod tests;
