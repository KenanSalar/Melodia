//! A `.lrc` sitting beside the audio file.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::lrc;
use melodia_core::entities::lyrics::{Lyrics, LyricsSource};
use melodia_core::error::AppError;

/// Reads whichever sidecar is there, or `None`.
///
/// A missing file is not a failure and is the overwhelmingly common answer; anything else that
/// went wrong is reported, including a sheet in an encoding that is not UTF-8, which
/// [`std::fs::read_to_string`] refuses. The caller decides what a report is worth.
pub(super) fn read(path: &Path) -> Result<Option<Lyrics>, AppError> {
    for candidate in candidates(path) {
        match std::fs::read_to_string(&candidate) {
            Ok(text) => {
                if let Some(lyrics) = lrc::parse(&text, LyricsSource::Sidecar) {
                    return Ok(Some(lyrics));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::io_source(e)),
        }
    }
    Ok(None)
}

/// Both sidecar names, in the order a tagger is likelier to have written.
///
/// `song.lrc` replaces the extension and `song.mp3.lrc` appends to it. Both are in circulation and
/// which one a user has depends on whatever produced it, so checking a single name is a coin flip.
///
/// Spelled rather than searched: matching a `.LRC` on a case-sensitive filesystem would mean
/// listing the directory on every track change, which is a real cost on a large folder against a
/// spelling almost nothing writes.
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
