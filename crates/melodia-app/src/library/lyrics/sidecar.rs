//! A `.lrc` sitting beside the audio file.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::lrc;
use melodia_core::entities::lyrics::{Lyrics, LyricsSource};
use melodia_core::error::AppError;

/// Reads whichever sidecar is there, or `None` where there is none. Anything else that goes wrong
/// is reported, a sheet that isn't UTF-8 included.
pub(super) fn read(path: &Path) -> Result<Option<Lyrics>, AppError> {
    Ok(read_text(path)?.and_then(|text| lrc::parse(&text, LyricsSource::Sidecar)))
}

/// The sheet as its author wrote it. **Blank counts as absent**, so an empty first candidate
/// doesn't hide a second one with the words.
pub(super) fn read_text(path: &Path) -> Result<Option<String>, AppError> {
    for candidate in candidates(path) {
        match std::fs::read_to_string(&candidate) {
            Ok(text) if !text.trim().is_empty() => return Ok(Some(text)),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::io_source(e)),
        }
    }
    Ok(None)
}

/// Both sidecar names in circulation, `song.lrc` then `song.mp3.lrc`.
///
/// Spelled rather than searched: matching a `.LRC` on a case-sensitive filesystem would list the
/// folder on every track change, for a spelling almost nothing writes.
fn candidates(path: &Path) -> impl Iterator<Item = PathBuf> {
    let appended = path.file_name().map(|name| {
        let mut name = name.to_os_string();
        name.push(".lrc");
        path.with_file_name(name)
    });
    std::iter::once(path.with_extension("lrc")).chain(appended)
}

#[cfg(test)]
#[path = "tests/sidecar_tests.rs"]
mod tests;
