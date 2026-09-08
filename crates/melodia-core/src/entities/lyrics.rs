//! What a lyric sheet is, independent of where it came from.
//!
//! `library::lyrics` resolves one of these out of a sidecar, a tag or the network, and the Now
//! Playing panel renders it. Neither end owns the vocabulary, and the panel's crate can name
//! neither of the other two, so it sits here.

/// One line, timed or not.
///
/// `at_ms` is `None` for a sheet carrying no timing at all, not for an odd line inside a timed
/// one: the parser keeps a sheet whole, so a mix never reaches here. Non-negative, since an
/// `[offset:]` that would push an early stamp below zero clamps instead; there is nowhere before
/// the start to seek to.
///
/// `end_ms` is where the sheet says the singing stops, which is a different question from where
/// the next line starts and is answered far less often: `None` means the sheet did not say, not
/// that the line runs to its successor. Only a sheet that spells it can tell a pause apart from a
/// long line, which is what the panel draws its instrumental breaks from.
///
/// `translation` is the gloss a bilingual sheet carries under the words. Two fields rather than
/// one string with a separator in it: the panel draws them at different sizes and the parser is
/// the half that knows how the sheet spelled the break.
///
/// `romanization` is the same words in Latin letters, and unlike the gloss it is derived rather
/// than read: `library::lyrics::romanize` fills it for a line written in a script it can sound
/// out, and leaves it `None` for every Latin one. The panel draws the three in the order they are
/// declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub at_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub text: String,
    pub romanization: Option<String>,
    pub translation: Option<String>,
}

/// Which arm of the resolver answered.
///
/// Kept for two questions the resolver cannot answer from the lines alone: whether a sheet is a
/// candidate for the on-disk store, which only [`LyricsSource::Online`] is, and where an
/// unexpected one came from when it reaches a log line.
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
    /// Builds a sheet, or `None` where there is nothing to show.
    ///
    /// Every resolver arm ends here, so "blank means no answer" is settled once rather than three
    /// times. Blank rather than empty: a tag holding only newlines is one a tagger created and
    /// nobody filled in, and drawing it hands the user a bare panel that claims to have found
    /// something. Blank lines *inside* a plain sheet are kept, being how one spaces its verses; a
    /// timed sheet reaches here with none, its blank stamps having closed the lines they end.
    #[must_use]
    pub fn new(lines: Vec<LyricLine>, source: LyricsSource) -> Option<Self> {
        if lines.iter().all(|line| line.text.trim().is_empty()) {
            return None;
        }
        Some(Self { lines, source })
    }

    /// Whether the panel can follow the song rather than only print it.
    ///
    /// Derived rather than stored: a `synced` field is a second answer to a question the lines
    /// already settle, and two answers can drift apart.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        self.lines.iter().any(|line| line.at_ms.is_some())
    }
}

/// What asking "does this track have lyrics" came to.
///
/// Three states rather than an `Option<Lyrics>`, because the store has to tell the last two apart:
/// a recording the directory *knows* has no words should never be asked about again, where one it
/// simply has not got deserves another try once its contributors have caught up. The panel draws
/// them differently too, one saying the song is instrumental and the other that nothing was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LyricsOutcome {
    Sheet(Lyrics),
    /// The directory says this recording has no words.
    Instrumental,
    /// Nobody has a sheet for it.
    Absent,
    /// The directory could not be asked, or refused to answer.
    ///
    /// **Not a fact about the recording**, which is the whole reason it is not [`Self::Absent`].
    /// The other three are answers and this is the absence of one, so nothing may record it: a
    /// refusal written down as "nobody has this" would suppress the track for as long as a real
    /// miss stands, on the strength of a bad minute.
    Unavailable,
}

/// What a lyrics directory said about one track.
///
/// The boundary between the crate that makes the request and the crate that decides what to do
/// with it, so the service's own response shape stays private to the former, as the radio
/// directory's does.
///
/// **Three outcomes, not two.** A sheet, nothing, or a track the directory knows has no words at
/// all. The last is a *positive* answer: it deserves copy that says so and it should never be
/// asked again, where "nothing found" is a different sentence and a different retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricsAnswer {
    /// LRC text, where the directory has a timed sheet.
    pub synced: Option<String>,
    /// Plain text, where it has only that.
    pub plain: Option<String>,
    pub instrumental: bool,
}

impl LyricsAnswer {
    /// The text worth keeping, timed in preference to plain.
    ///
    /// One place decides that preference, so the copy written to the store and the copy handed to
    /// the parser cannot disagree about which sheet arrived.
    ///
    /// **Each field is judged before it is preferred.** A directory that answers with an empty
    /// `syncedLyrics` beside a real `plainLyrics` is answering with the plain one, and picking the
    /// timed field first and testing it afterwards would throw that away.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        filled(self.synced.as_deref()).or_else(|| filled(self.plain.as_deref()))
    }
}

/// A field that is there rather than present and blank.
fn filled(field: Option<&str>) -> Option<&str> {
    field.map(str::trim).filter(|text| !text.is_empty())
}

#[cfg(test)]
#[path = "tests/lyrics_tests.rs"]
mod tests;
