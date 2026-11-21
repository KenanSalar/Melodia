//! What a lyric sheet is, independent of where it came from: `library::lyrics` resolves one and
//! the Now Playing panel draws it, and the panel's crate can name neither.

use crate::utils::text::filled;

/// One line, timed or not. `at_ms` is `None` only for a sheet with no timing at all, and clamped
/// non-negative. `end_ms` is where the sheet says the singing stops, by a blank stamp after the
/// line or a stamp after its last word, `None` where it didn't say, which is how the panel tells a
/// pause from a long line. `words` runs in time order from `at_ms`, empty for a line lit whole.
/// `translation` is a bilingual sheet's gloss, apart because the panel draws it smaller;
/// `romanization` is derived by `library::lyrics::romanize`. The panel draws the three in
/// declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub at_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub text: String,
    pub romanization: Option<String>,
    pub translation: Option<String>,
    pub words: Vec<WordStamp>,
}

/// When a word, or a syllable, lights. `start` is its first byte in the line's `text`, so what is
/// lit is the text up to the first word not yet due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordStamp {
    pub start: usize,
    pub at_ms: i64,
}

/// How closely a sheet can be followed, ordered so the best of several is the greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Timing {
    Plain,
    Lines,
    Words,
}

/// Which arm answered: only [`LyricsSource::Online`] is a candidate for the on-disk store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsSource {
    /// A `.lrc` beside the audio file.
    Sidecar,
    /// The file's own lyrics tag.
    Tag,
    /// Looked up over the network.
    Online,
}

/// A whole sheet, guaranteed to have something to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    pub source: LyricsSource,
}

impl Lyrics {
    /// `None` where every line is blank, settled once for every arm: a tag holding only newlines
    /// is one a tagger created and nobody filled. Blank lines inside a plain sheet are kept,
    /// spacing its verses.
    #[must_use]
    pub fn new(lines: Vec<LyricLine>, source: LyricsSource) -> Option<Self> {
        if lines.iter().all(|line| line.text.trim().is_empty()) {
            return None;
        }
        Some(Self { lines, source })
    }

    /// Derived rather than stored, so it can't drift from the lines.
    #[must_use]
    pub fn timing(&self) -> Timing {
        if self.lines.iter().any(|line| !line.words.is_empty()) {
            Timing::Words
        } else if self.lines.iter().any(|line| line.at_ms.is_some()) {
            Timing::Lines
        } else {
            Timing::Plain
        }
    }

    /// Whether the panel follows the sheet as it plays, by lines at least.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        self.timing() != Timing::Plain
    }
}

/// The store never asks about an instrumental again, retries an absent one once contributors
/// have caught up, and records nothing for [`Self::Unavailable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsOutcome {
    Sheet(Lyrics),
    /// The directory says this recording has no words.
    Instrumental,
    /// Nobody has a sheet for it.
    Absent,
    /// The directory couldn't be asked, or refused. Not a fact about the recording, so recording
    /// it would suppress the track on the strength of a bad minute.
    Unavailable,
}

/// What a lyrics directory said about one track, keeping the service's response shape private to
/// the crate that asked. `instrumental` is a positive answer, never asked again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsAnswer {
    /// LRC text, where the directory has a timed sheet, its words timed where the directory
    /// times them.
    pub synced: Option<String>,
    /// Plain text, where it has only that.
    pub plain: Option<String>,
    pub instrumental: bool,
}

impl LyricsAnswer {
    /// Timed in preference to plain, decided once so the stored and the parsed copy agree. Each
    /// field is judged before it is preferred: an empty `syncedLyrics` beside a real
    /// `plainLyrics` answers with the plain one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        filled(self.synced.as_deref()).or_else(|| filled(self.plain.as_deref()))
    }

    /// Judged as [`Self::text`] judges it, or a blank `synced` would end a search on a sheet
    /// `text` then declines.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        filled(self.synced.as_deref()).is_some()
    }

    /// Read off the text, the directories carrying no such column.
    #[must_use]
    pub fn has_gloss(&self) -> bool {
        self.text().is_some_and(carries_gloss)
    }
}

/// What a bilingual sheet puts between a line and its gloss, no part of the format. Here because
/// the client choosing among a recording's uploads asks about a gloss of raw text, before anything
/// is parsed.
pub const TRANSLATION_MARK: char = '^';

/// A line's words and its gloss, cut at [`gloss_caret`].
#[must_use]
pub fn split_gloss(line: &str) -> (&str, Option<&str>) {
    let Some(caret) = gloss_caret(line) else {
        return (line, None);
    };
    let (sung, rest) = line.split_at(caret);
    let gloss = &rest[TRANSLATION_MARK.len_utf8()..];
    (sung.trim_end(), Some(gloss.trim()))
}

/// Where [`split_gloss`] cuts a line, for a caller holding offsets into it whole. Only a caret with
/// text either side separates; anywhere else it is a character, and kept.
#[must_use]
pub fn gloss_caret(line: &str) -> Option<usize> {
    let (sung, gloss) = line.split_once(TRANSLATION_MARK)?;
    let separates = !sung.trim_end().is_empty() && !gloss.trim().is_empty();
    separates.then_some(sung.len())
}

/// Asked of raw LRC, stamps and all, so it answers before a sheet is parsed.
#[must_use]
pub fn carries_gloss(text: &str) -> bool {
    sheet_lines(text).any(|line| gloss_caret(line).is_some())
}

/// A sheet's lines, broken at `\n`, `\r\n` or a lone `\r`, the classic Mac OS break some taggers
/// still write into an M4A. A `\r` closing the text, or doubled in `\r\r\n`, adds no blank line.
pub fn sheet_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().flat_map(|line| line.trim_suffix('\r').split('\r'))
}

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
