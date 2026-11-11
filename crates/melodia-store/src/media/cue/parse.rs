//! A cue sheet's text as the sheet and tracks it states, read the way rippers write sheets rather
//! than the way the CDRWIN appendix specifies them: keywords in any case, quotes optional, minutes
//! past 99, junk after the arguments ignored. A line that can't be read is dropped alone.

use std::path::Path;

use melodia_core::entities::cue::{CueFrames, CueTrack, FRAMES_PER_SECOND};

#[derive(Default)]
pub(crate) struct Sheet {
    pub(crate) fields: Fields,
    pub(crate) catalog: Option<String>,
    /// Each `FILE` line's name as written, in sheet order, which an [`Index`]'s `file` counts in.
    pub(crate) files: Vec<String>,
    pub(crate) tracks: Vec<SheetTrack>,
}

impl Sheet {
    /// Whether any track says where it starts, as every sheet that describes a disc does.
    pub(crate) fn places_a_track(&self) -> bool {
        self.tracks.iter().any(|track| track.index(1).is_some())
    }

    pub(crate) fn track(&self, number: CueTrack) -> Option<&SheetTrack> {
        self.tracks.iter().find(|track| track.number == number)
    }

    pub(crate) fn track_mut(&mut self, number: CueTrack) -> Option<&mut SheetTrack> {
        self.tracks.iter_mut().find(|track| track.number == number)
    }
}

/// The `REM` keys a sheet's tags are read from and written under.
pub(crate) mod rem {
    pub(crate) const COMPOSER: &str = "COMPOSER";
    pub(crate) const DATE: &str = "DATE";
    pub(crate) const DISCNUMBER: &str = "DISCNUMBER";
    pub(crate) const GENRE: &str = "GENRE";
    pub(crate) const TOTALDISCS: &str = "TOTALDISCS";
}

/// What a sheet states for itself, or for one track. A blank value is no value.
#[derive(Default)]
pub(crate) struct Fields {
    pub(crate) title: Option<String>,
    pub(crate) performer: Option<String>,
    pub(crate) songwriter: Option<String>,
    /// `REM` lines, the key upper-cased.
    rems: Vec<(String, String)>,
}

impl Fields {
    pub(crate) fn rem(&self, key: &str) -> Option<&str> {
        self.rems.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }

    /// States `value` under the upper-cased `key` in place of whatever the lines did.
    pub(crate) fn set_rem(&mut self, key: &str, value: String) {
        self.rems.retain(|(name, _)| name != key);
        self.rems.push((key.to_owned(), value));
    }
}

pub(crate) struct SheetTrack {
    pub(crate) number: CueTrack,
    pub(crate) is_audio: bool,
    pub(crate) fields: Fields,
    pub(crate) isrc: Option<String>,
    indexes: Vec<Index>,
}

impl SheetTrack {
    /// A track of the one file a sheet is inside, at the index points `indexes` numbers.
    pub(crate) fn inside(
        number: CueTrack,
        is_audio: bool,
        isrc: Option<String>,
        indexes: impl IntoIterator<Item = (u8, CueFrames)>,
    ) -> Self {
        let indexes = indexes.into_iter().map(|(number, at)| Index { number, at, file: 0 });
        Self { number, is_audio, fields: Fields::default(), isrc, indexes: indexes.collect() }
    }

    pub(crate) fn index(&self, number: u8) -> Option<Index> {
        self.indexes.iter().find(|index| index.number == number).copied()
    }
}

/// What a sheet's `FILE` lines name.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Files {
    /// The audio files the lines under each place tracks in, as a `.cue` beside them names them.
    Named,
    /// Nothing: the sheet is inside the one file it splits, whatever file the ripper's lines name.
    Inside,
}

impl Files {
    /// The files a sheet starts out naming, which an [`Index`] counts in.
    pub(super) fn at_start(self) -> Vec<String> {
        match self {
            Self::Named => Vec::new(),
            Self::Inside => vec![String::new()],
        }
    }
}

/// One `INDEX` line: where in which file a track's index point sits.
#[derive(Clone, Copy)]
pub(crate) struct Index {
    number: u8,
    pub(crate) at: CueFrames,
    /// The `FILE` the line sits under, a position in [`Sheet::files`].
    pub(crate) file: usize,
}

/// Reads `text`, logging each line it drops against `origin`.
pub(crate) fn parse(text: &str, origin: &Path, files: Files) -> Sheet {
    let mut reader = Reader::new(files);
    for (line_number, line) in text.lines().enumerate() {
        if reader.read(line).is_err() {
            log::warn!("Skipped line {} of {}: {}", line_number + 1, origin.display(), line.trim());
        }
    }
    reader.sheet
}

/// What each of `lines` states, in order, beside the sheet they state together. A line that can't
/// be read states [`Command::Other`].
pub(super) fn statements<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    files: Files,
) -> (Sheet, Vec<Statement>) {
    let mut reader = Reader::new(files);
    let statements = lines
        .into_iter()
        .map(|line| {
            let command = reader.read(line).unwrap_or(Command::Other);
            Statement { scope: reader.scope(), command }
        })
        .collect();
    (reader.sheet, statements)
}

/// The value `line` states, read as the sheet's reader reads it.
pub(super) fn stated_value(line: &str) -> Option<String> {
    let (keyword, rest) = split_word(line);
    let rest = if keyword.eq_ignore_ascii_case("REM") { split_word(rest).1 } else { rest };
    value_of(rest)
}

/// One line: what it says, and whose it is.
pub(super) struct Statement {
    pub(super) scope: Scope,
    pub(super) command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// Ahead of the first `TRACK`.
    Sheet,
    /// A `TRACK` line and the lines under it.
    Track(CueTrack),
    /// The lines under a `TRACK` line that couldn't be read.
    Rejected,
}

#[derive(PartialEq, Eq)]
pub(super) enum Command {
    File,
    Track,
    Title,
    Performer,
    Songwriter,
    Isrc,
    Catalog,
    /// The key upper-cased.
    Rem(String),
    /// Any line no field is read from.
    Other,
}

/// A line that names something the sheet can't hold, such as an index with no file above it.
struct Unreadable;

struct Reader {
    sheet: Sheet,
    files: Files,
    /// Set by a `TRACK` line that couldn't be read, so its lines aren't taken for the track before.
    in_rejected_track: bool,
}

impl Reader {
    fn new(files: Files) -> Self {
        let sheet = Sheet { files: files.at_start(), ..Sheet::default() };
        Self { sheet, files, in_rejected_track: false }
    }

    fn read(&mut self, line: &str) -> Result<Command, Unreadable> {
        let (keyword, rest) = split_word(line);
        match keyword.to_ascii_uppercase().as_str() {
            "FILE" if self.files == Files::Inside => Ok(Command::File),
            "FILE" => {
                let name = file_name(rest).ok_or(Unreadable)?;
                // Some rippers repeat the image's line ahead of every track, which still names one
                // file whose tracks end one another.
                if self.sheet.files.last() != Some(&name) {
                    self.sheet.files.push(name);
                }
                Ok(Command::File)
            }
            "TRACK" => self.start_track(rest).map(|()| Command::Track),
            _ if self.in_rejected_track => Ok(Command::Other),
            "INDEX" => self.add_index(rest).map(|()| Command::Other),
            "TITLE" => {
                set(&mut self.fields().title, rest);
                Ok(Command::Title)
            }
            "PERFORMER" => {
                set(&mut self.fields().performer, rest);
                Ok(Command::Performer)
            }
            "SONGWRITER" => {
                set(&mut self.fields().songwriter, rest);
                Ok(Command::Songwriter)
            }
            "REM" => {
                let (key, value) = split_word(rest);
                let key = key.to_ascii_uppercase();
                if let Some(value) = value_of(value) {
                    self.fields().rems.push((key.clone(), value));
                }
                Ok(Command::Rem(key))
            }
            "ISRC" => {
                if let Some(track) = self.sheet.tracks.last_mut() {
                    set(&mut track.isrc, rest);
                }
                Ok(Command::Isrc)
            }
            "CATALOG" => {
                set(&mut self.sheet.catalog, rest);
                Ok(Command::Catalog)
            }
            // `FLAGS`, `PREGAP`, `POSTGAP`, `CDTEXTFILE` and the unknown say nothing a row holds.
            _ => Ok(Command::Other),
        }
    }

    fn scope(&self) -> Scope {
        if self.in_rejected_track {
            return Scope::Rejected;
        }
        self.sheet.tracks.last().map_or(Scope::Sheet, |track| Scope::Track(track.number))
    }

    /// The fields a line goes to: the last track's, or the sheet's ahead of the first `TRACK`.
    fn fields(&mut self) -> &mut Fields {
        match self.sheet.tracks.last_mut() {
            Some(track) => &mut track.fields,
            None => &mut self.sheet.fields,
        }
    }

    /// A track numbered 1 to 99, its mode `AUDIO` when it states none.
    fn start_track(&mut self, rest: &str) -> Result<(), Unreadable> {
        let (number, rest) = split_word(rest);
        let (mode, _) = split_word(rest);
        let number = number
            .parse::<i64>()
            .ok()
            .and_then(|number| CueTrack::try_from(number).ok())
            .filter(|&number| number != CueTrack::HIDDEN);
        self.in_rejected_track = number.is_none();
        let number = number.ok_or(Unreadable)?;
        self.sheet.tracks.push(SheetTrack {
            number,
            is_audio: mode.is_empty() || mode.eq_ignore_ascii_case("AUDIO"),
            fields: Fields::default(),
            isrc: None,
            indexes: Vec::new(),
        });
        Ok(())
    }

    fn add_index(&mut self, rest: &str) -> Result<(), Unreadable> {
        let (number, rest) = split_word(rest);
        let (time, _) = split_word(rest);
        let number = number.parse::<u8>().map_err(|_| Unreadable)?;
        let at = parse_time(time).ok_or(Unreadable)?;
        let file = self.sheet.files.len().checked_sub(1).ok_or(Unreadable)?;
        let track = self.sheet.tracks.last_mut().ok_or(Unreadable)?;
        track.indexes.push(Index { number, at, file });
        Ok(())
    }
}

/// `mm:ss:ff`, the minutes unbounded since a long image runs past the 99 the format allows.
fn parse_time(text: &str) -> Option<CueFrames> {
    let mut parts = text.split(':').map(str::parse::<u32>);
    let (minutes, seconds, frames) =
        (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
    if parts.next().is_some() || seconds >= 60 || frames >= FRAMES_PER_SECOND {
        return None;
    }
    let total = minutes.checked_mul(60)?.checked_add(seconds)?;
    let total = total.checked_mul(FRAMES_PER_SECOND)?.checked_add(frames)?;
    CueFrames::try_from(i64::from(total)).ok()
}

/// `text`'s first word and what follows it, both trimmed.
fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim();
    text.split_once(char::is_whitespace).map_or((text, ""), |(word, rest)| (word, rest.trim()))
}

/// The value a command states: between its first quote and its last, or the whole rest unquoted.
fn value_of(rest: &str) -> Option<String> {
    let value = quoted(rest).unwrap_or(rest).trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// A `FILE` line's name. Unquoted, the type keyword is its last word and the name all before it.
fn file_name(rest: &str) -> Option<String> {
    let name = quoted(rest)
        .unwrap_or_else(|| rest.rsplit_once(char::is_whitespace).map_or(rest, |(name, _)| name))
        .trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// The text after an opening quote, up to the last quote on the line: a title can quote a word.
fn quoted(rest: &str) -> Option<&str> {
    let inner = rest.strip_prefix('"')?;
    Some(inner.rfind('"').map_or(inner, |end| &inner[..end]))
}

/// Keeps a value already read over a blank repeat of its line.
fn set(field: &mut Option<String>, rest: &str) {
    if let Some(value) = value_of(rest) {
        *field = Some(value);
    }
}
