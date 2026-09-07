//! The file's own lyrics tag.

use std::path::Path;

use super::lrc;
use crate::library::tags;
use melodia_core::entities::lyrics::{Lyrics, LyricsSource};
use melodia_core::error::AppError;

/// Reads the lyrics tag and parses whatever it holds.
///
/// The tag is one string and the format is not declared anywhere in it: `LYRICS` on Vorbis and
/// `USLT` on `ID3v2` are both routinely filled with LRC text and just as routinely with plain
/// prose. [`lrc::parse`] answers either without being asked which, so there is no sniffing step
/// and no way for the two to disagree about what a tag contained.
pub(super) fn read(path: &Path) -> Result<Option<Lyrics>, AppError> {
    let Some(text) = tags::read_lyrics(path)? else {
        return Ok(None);
    };
    Ok(lrc::parse(&text, LyricsSource::Tag))
}
