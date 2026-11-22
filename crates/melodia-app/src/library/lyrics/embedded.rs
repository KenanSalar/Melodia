//! The file's own lyrics tag, which can hold several entries, a `USLT` per language or a second
//! `LYRICS`: [`best`] is the one choice of which shows, so the panel and the tag editor agree.

use std::path::Path;

use super::lrc;
use crate::library::tags;
use melodia_core::entities::lyrics::{Lyrics, LyricsSource, Timing};
use melodia_core::error::AppError;

/// Reads the lyrics tag and parses its best entry, LRC or plain prose, the tag declaring neither.
pub(super) fn read(path: &Path) -> Result<Option<Lyrics>, AppError> {
    let entries = tags::read_lyrics(path)?;
    Ok(best(&entries).and_then(|entry| entry.sheet))
}

/// [`read`]'s entry as the tag held it.
pub(super) fn read_text(path: &Path) -> Result<Option<String>, AppError> {
    let entries = tags::read_lyrics(path)?;
    Ok(best_tag_entry(&entries).map(str::to_owned))
}

/// Which of a lyrics tag's entries shows, as the tag held it.
#[must_use]
pub fn best_tag_entry(entries: &[String]) -> Option<&str> {
    best(entries).map(|entry| entry.text)
}

struct Entry<'a> {
    text: &'a str,
    sheet: Option<Lyrics>,
}

/// Word times over line times over plain text, the first on a tie, and an entry nothing parses
/// from last.
fn best(entries: &[String]) -> Option<Entry<'_>> {
    let mut chosen: Option<(Option<Timing>, Entry<'_>)> = None;
    for text in entries {
        let sheet = lrc::parse(text, LyricsSource::Tag);
        let timing = sheet.as_ref().map(Lyrics::timing);
        let entry = Entry { text, sheet };
        if timing == Some(Timing::Words) {
            return Some(entry);
        }
        if chosen.as_ref().is_none_or(|(kept, _)| timing > *kept) {
            chosen = Some((timing, entry));
        }
    }
    chosen.map(|(_, entry)| entry)
}

#[cfg(test)]
#[path = "tests/embedded_tests.rs"]
mod tests;
