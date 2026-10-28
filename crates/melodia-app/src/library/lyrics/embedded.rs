//! The file's own lyrics tag.

use std::path::Path;

use super::lrc;
use crate::library::tags;
use melodia_core::entities::lyrics::{Lyrics, LyricsSource};
use melodia_core::error::AppError;

/// Reads the lyrics tag and parses whatever it holds, LRC or plain prose, the tag declaring
/// neither.
pub(super) fn read(path: &Path) -> Result<Option<Lyrics>, AppError> {
    let Some(text) = tags::read_lyrics(path)? else {
        return Ok(None);
    };
    Ok(lrc::parse(&text, LyricsSource::Tag))
}
