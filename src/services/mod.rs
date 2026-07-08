pub mod always_on_top;
pub mod artist_images;
#[cfg(target_os = "linux")]
pub mod desktop_integration;
#[cfg(target_os = "windows")]
pub mod dwm_titlebar;
pub mod material_you;
pub mod media_controls;
pub mod search_history;
pub mod settings;
#[cfg(target_os = "linux")]
pub mod system_theme;
pub mod toast;
pub mod tray;
pub mod updater;
pub mod view_state;

use std::io::{BufWriter, Write};
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{AppError, AppResult};

/// Read JSON from `path` and deserialize to `T`. Missing file or parse error
/// fall back to `T::default()` (parse errors are logged at warn level). Sync
/// variant — used at startup before the tokio runtime exists.
pub fn load_json_or_default_sync<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!(
            "Failed to parse {}, using defaults: {e}",
            path.display()
        );
        T::default()
    }))
}

/// Async variant of [`load_json_or_default_sync`]. Both missing-file and
/// parse-error paths fall back to `T::default()`.
pub async fn load_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> AppResult<T> {
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return Ok(T::default());
    };
    Ok(serde_json::from_str::<T>(&content).unwrap_or_else(|e| {
        log::warn!(
            "Failed to parse {}, using defaults: {e}",
            path.display()
        );
        T::default()
    }))
}

/// Atomically write `value` as pretty JSON to `path` by streaming through a
/// temp file in the same directory and renaming on success. Avoids the
/// allocate-entire-payload-as-String pattern of `to_string_pretty` + `write`,
/// and a crash mid-write leaves the previous file intact.
pub fn write_json_atomic_sync<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut writer = BufWriter::new(tmp.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value)
            .map_err(AppError::io_source)?;
        writer.flush()?;
    }
    tmp.persist(path).map_err(|e| AppError::Io(e.error))?;
    Ok(())
}

/// Atomically write plain `text` to `path` via a temp file in the same
/// directory + rename on success. Plain-text sibling of
/// [`write_json_atomic_sync`] (used by M3U playlist export). Bytes are
/// written verbatim — the caller owns line endings and trailing newline.
pub fn write_text_atomic_sync(path: &Path, text: &str) -> AppResult<()> {
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
