//! Whole-file read and atomic whole-file write: the JSON and text beside the database, and a
//! track's tag rewrite. The reads need no lock because every write lands through a same-directory
//! temp file and a rename, so a reader only ever sees one whole file and a crash or power cut
//! leaves one. The temp is flushed before the rename and, on unix, its folder after, so on unix a
//! save that has returned survives a power cut too. A temp still being written when `main` exits
//! is removed then ([`remove_unfinished`]).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::mem::DropGuard;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tempfile::NamedTempFile;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::error::{AppError, AppResult, describe};

/// JSON from `path`, `T::default()` on a missing file; sync, for before the runtime exists.
///
/// A value this build can't read (a newer build's token, a bad hand edit) resets alone, where read
/// whole it would default the file and the next write would persist that. Leans on `T`'s
/// `#[serde(default)]`; only a file that isn't a JSON object falls back whole.
pub fn load_json_or_default_sync<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(parse_resetting_unreadable(&content, path))
}

/// [`load_json_or_default_sync`]'s async twin.
pub async fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok(T::default());
    };
    Ok(parse_resetting_unreadable(&content, path))
}

/// Asked a top-level field at a time, because serde reads a `#[serde(flatten)]` struct from a
/// buffered copy no deserializer wrapper sees into, and `settings.json` is flattened flag structs.
/// Logs which fields went, never what they held: a scrobbler's session key is read here.
fn parse_resetting_unreadable<T: DeserializeOwned + Default>(content: &str, path: &Path) -> T {
    if let Ok(parsed) = serde_json::from_str(content) {
        return parsed;
    }
    let fields = match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(fields)) => fields,
        Ok(_) => {
            log::warn!("Failed to read {}, using defaults", path.display());
            return T::default();
        }
        Err(e) => {
            log::warn!("Failed to parse {}, using defaults: {}", path.display(), describe(&e));
            return T::default();
        }
    };
    let (readable, unreadable): (Map<String, Value>, Map<String, Value>) =
        fields.into_iter().partition(|(key, value)| reads_alone::<T>(key, value));
    let Ok(salvaged) = serde_json::from_value(Value::Object(readable)) else {
        log::warn!("Failed to read {}, using defaults", path.display());
        return T::default();
    };
    // Empty for a file whose only fault was a duplicate key, the `Value` having kept the last.
    if !unreadable.is_empty() {
        let reset = unreadable.keys().map(String::as_str).collect::<Vec<_>>().join(", ");
        log::warn!("{}: {reset} won't read, so they take their defaults", path.display());
    }
    salvaged
}

fn reads_alone<T: DeserializeOwned>(key: &str, value: &Value) -> bool {
    let alone = Map::from_iter([(key.to_owned(), value.clone())]);
    T::deserialize(&Value::Object(alone)).is_ok()
}

/// Pretty JSON through [`write_with_sync`], never holding the payload as a `String`.
pub fn write_json_sync<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    write_with_sync(path, |writer| {
        serde_json::to_writer_pretty(writer, value).map_err(AppError::io_source)
    })
}

/// Verbatim: the caller owns line endings and the trailing newline.
pub fn write_text_sync(path: &Path, text: &str) -> AppResult<()> {
    write_with_sync(path, |writer| Ok(writer.write_all(text.as_bytes())?))
}

/// [`write_text_sync`] on the blocking pool.
pub async fn write_text(path: PathBuf, text: String) -> AppResult<()> {
    tokio::task::spawn_blocking(move || write_text_sync(&path, &text))
        .await
        .map_err(AppError::io_source)?
}

/// Hands `write` a same-directory temp file, renamed over `path` once `write` succeeds. The writer
/// seeks as well as writes, which a zip needs to fill in each entry's header.
pub fn write_with_sync(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<&mut File>) -> AppResult<()>,
) -> AppResult<()> {
    let folder = folder_of(path);
    std::fs::create_dir_all(folder)?;
    let (_unfinished, mut tmp) = unfinished_temp(folder)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        write(&mut writer)?;
        writer.flush()?;
    }
    tmp.as_file().sync_all()?;
    rename_over(tmp.into_temp_path(), path)
}

/// Hands `rewrite` a copy of `path`, as a read-write handle and its path, renamed over the original
/// once `rewrite` succeeds. A rewrite in place moves every byte after the edit under whoever has
/// the file open, so a deck playing the track would read the wrong offsets; a rename leaves it on
/// the file it opened. Costs one copy and write permission on the directory, with no in-place
/// fallback: a second write path is the one nobody exercises.
///
/// `rewrite` writes through the handle, never a second open of the path: a write the disk or share
/// refuses can surface only at the writer's close, whose error `Drop` discards, so the flush that
/// reports it runs on the handle that wrote.
pub fn rewrite_with_sync(
    path: &Path,
    rewrite: impl FnOnce(&mut File, &Path) -> AppResult<()>,
) -> AppResult<()> {
    // Resolved first: a rename replaces a symlink where a write in place followed it.
    let path = &std::fs::canonicalize(path)?;
    // Closed for `fs::copy`, which opens it itself; no extension, so the watcher skips it.
    let (_unfinished, tmp) = unfinished_temp(folder_of(path))?;
    let tmp = tmp.into_temp_path();
    std::fs::copy(path, &tmp)?;
    {
        let mut copy = File::options().read(true).write(true).open(&tmp)?;
        rewrite(&mut copy, &tmp)?;
        copy.sync_all()?;
    }
    rename_over(tmp, path)
}

/// Renames `tmp` over `path`, removing it if the rename fails. Not `TempPath::persist`, a bare
/// `MoveFileExW` on Windows that can't take a name a playing deck holds open; `std::fs::rename`
/// retries with POSIX semantics, which can. A handle denying delete sharing still fails the write
/// rather than corrupting it.
///
/// [`TempPath::keep`] first, clearing the `FILE_ATTRIBUTE_TEMPORARY` the temp was created with, or
/// every file written here tells the cache manager not to flush it.
///
/// [`TempPath::keep`]: tempfile::TempPath::keep
fn rename_over(tmp: tempfile::TempPath, path: &Path) -> AppResult<()> {
    let tmp = tmp.keep().map_err(|e| AppError::Io(e.error))?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        // `keep` disarmed the cleanup, and there is nothing to report past the rename's own error.
        let _ = std::fs::remove_file(&tmp);
    })?;
    #[cfg(unix)]
    sync_parent(path);
    Ok(())
}

/// Flushes the folder holding `path`, so a rename into it survives a power cut. Logged rather than
/// returned: the rename has landed by then.
#[cfg(unix)]
pub fn sync_parent(path: &Path) {
    let folder = folder_of(path);
    let Err(e) = File::open(folder).and_then(|handle| handle.sync_all()) else {
        return;
    };
    // A filesystem that can't flush a folder says so on every write, thousands in a batch edit.
    if matches!(e.kind(), std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput) {
        log::debug!("{} can't be flushed: {}", folder.display(), describe(&e));
    } else {
        log::warn!("Failed to flush {}: {}", folder.display(), describe(&e));
    }
}

/// The temps of writes still running.
static UNFINISHED: Mutex<Unfinished> = Mutex::new(Unfinished { closed: false, temps: Vec::new() });

struct Unfinished {
    /// Set by [`remove_unfinished`], after which no write starts.
    closed: bool,
    temps: Vec<PathBuf>,
}

fn unfinished() -> MutexGuard<'static, Unfinished> {
    UNFINISHED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Unlists its temp from [`UNFINISHED`] once dropped.
type Unlisting = DropGuard<PathBuf, fn(PathBuf)>;

/// A temp in `folder`, listed as unfinished until the guard drops. Bound guard first, so the temp
/// is gone or renamed before it is unlisted. Created outside the lock, which a slow disk would
/// hold for every other write, and refused once [`remove_unfinished`] has run, its drop removing it.
fn unfinished_temp(folder: &Path) -> AppResult<(Unlisting, NamedTempFile)> {
    let tmp = NamedTempFile::new_in(folder)?;
    let mut unfinished = unfinished();
    if unfinished.closed {
        return Err(AppError::io_other("Melodia is quitting, so no write starts"));
    }
    unfinished.temps.push(tmp.path().to_path_buf());
    let unlist: fn(PathBuf) = |tmp| finished(&tmp);
    Ok((DropGuard::new(tmp.path().to_path_buf(), unlist), tmp))
}

fn finished(tmp: &Path) {
    let mut unfinished = unfinished();
    if let Some(at) = unfinished.temps.iter().position(|held| held == tmp) {
        unfinished.temps.swap_remove(at);
    }
}

/// Removes the temps of writes still running and lets no other start, for `main`'s way out, which
/// runs no destructor: a write the shutdown budget cut short would leave its copy beside the file
/// for good. Each original stays whole, its rename never having run.
pub fn remove_unfinished() {
    let temps = {
        let mut unfinished = unfinished();
        unfinished.closed = true;
        std::mem::take(&mut unfinished.temps)
    };
    let mut removed = 0;
    for tmp in &temps {
        match std::fs::remove_file(tmp) {
            Ok(()) => removed += 1,
            // Renamed over its file since: the write landed.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("Failed to remove {}: {}", tmp.display(), describe(&e)),
        }
    }
    if removed > 0 {
        log::info!("Removed {removed} unfinished write(s) at exit");
    }
}

/// The folder `path` sits in, `.` for a bare file name, whose empty parent won't open.
fn folder_of(path: &Path) -> &Path {
    path.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

#[cfg(test)]
#[path = "tests/atomic_file_tests.rs"]
mod tests;
