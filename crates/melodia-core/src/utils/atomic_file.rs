//! Whole-file read and atomic whole-file write, for the small JSON and text files the app keeps
//! beside its database.
//!
//! The reads are plain and unsynchronised, and they are safe because the writes are not: every
//! write lands through a temp file in the same directory and a rename, so a reader sees either
//! the previous file entire or the new one entire, and a crash mid-write leaves the previous one
//! intact.

use std::io::{BufWriter, Write};
use std::path::Path;

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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value).map_err(AppError::io_source)?;
        writer.flush()?;
    }
    tmp.persist(path).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}

/// [`write_json_sync`]'s plain-text sibling, for M3U export. Bytes go out verbatim — the caller
/// owns line endings and the trailing newline.
pub fn write_text_sync(path: &Path, text: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        writer.write_all(text.as_bytes())?;
        writer.flush()?;
    }
    tmp.persist(path).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}
