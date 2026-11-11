//! Edit Tags on a cue sheet's tracks, written into the sheet at the scope each field belongs to:
//! the track's lines, or the sheet's ahead of its first `TRACK`.
//!
//! The sheet is rewritten line by line, never re-serialized: a line the edit doesn't touch is
//! copied as the file holds it, BOM and line ending included, so a codepage guessed wrong survives
//! where decoding and encoding it again would not. A written line takes the sheet's own encoding
//! where every character fits, and the whole sheet becomes UTF-8 with a BOM where one doesn't.

use std::path::Path;

use lofty::tag::items::Timestamp;

use super::embedded::TrackField;
use super::parse::{
    Command, Files, Scope, Sheet, SheetTrack, Statement, rem, stated_value, statements,
};
use super::text::{EncodedSheet, LineEnding, OutLine, RawLine};
use crate::media::ingest::metadata::LIST_SEPARATOR;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::credits::CreditRole;
use melodia_core::entities::cue::CueTrack;
use melodia_core::entities::tags::{FieldEdit, RoleCreditEdit, TagEdit};
use melodia_core::error::AppError;
use melodia_core::utils::atomic_file;

/// The part of a tag edit a cue sheet holds. The rest describes the whole disc, in the image's
/// tags, or lives in the database.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SheetEdit {
    title: FieldEdit<String>,
    artist: FieldEdit<String>,
    composer: FieldEdit<String>,
    isrc: FieldEdit<String>,
    genre: FieldEdit<String>,
    year: FieldEdit<u16>,
    album: FieldEdit<String>,
    album_artist: FieldEdit<String>,
    disc_number: FieldEdit<String>,
    disc_total: FieldEdit<String>,
    barcode: FieldEdit<String>,
}

impl SheetEdit {
    /// Whether the edit changes nothing a sheet holds, as a rating's write-back doesn't.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// A sheet holds one `PERFORMER` and one `SONGWRITER` per scope, so a credit is written as it
/// prints and several composers as one value the scan splits again.
impl From<&TagEdit> for SheetEdit {
    fn from(edit: &TagEdit) -> Self {
        Self {
            title: edit.title.clone(),
            artist: printed(&edit.artist),
            composer: composers(&edit.credits),
            isrc: edit.isrc.clone(),
            genre: mapped(&edit.genres, |genres| joined(genres.names().iter().map(String::as_str))),
            year: edit.year.clone(),
            album: edit.album.clone(),
            album_artist: printed(&edit.album_artist),
            disc_number: mapped(&edit.disc_number, |number| Some(number.to_string())),
            disc_total: mapped(&edit.disc_total, |total| Some(total.to_string())),
            barcode: edit.barcode.clone(),
        }
    }
}

/// `edit` with its value through `value`, a `None` from it clearing the field.
fn mapped<T, U>(edit: &FieldEdit<T>, value: impl FnOnce(&T) -> Option<U>) -> FieldEdit<U> {
    match edit {
        FieldEdit::Keep => FieldEdit::Keep,
        FieldEdit::Clear => FieldEdit::Clear,
        FieldEdit::Set(set) => value(set).map_or(FieldEdit::Clear, FieldEdit::Set),
    }
}

fn printed(credit: &FieldEdit<ArtistCredit>) -> FieldEdit<String> {
    mapped(credit, |credit| credit.line().map(str::to_owned))
}

/// The composers, kept where the edit's credits leave the role out of scope.
fn composers(credits: &FieldEdit<RoleCreditEdit>) -> FieldEdit<String> {
    if let FieldEdit::Set(edit) = credits
        && !edit.scope().any(|role| role == CreditRole::Composer)
    {
        return FieldEdit::Keep;
    }
    mapped(credits, |edit| {
        let credits = edit.credits().all().iter();
        let composers = credits.filter(|credit| credit.role == CreditRole::Composer);
        joined(composers.map(|composer| composer.name.as_str()))
    })
}

fn joined<'a>(names: impl Iterator<Item = &'a str>) -> Option<String> {
    let names: Vec<&str> = names.collect();
    (!names.is_empty()).then(|| names.join(&format!("{LIST_SEPARATOR} ")))
}

/// One sheet's tracks as an edit meets them.
#[derive(Default)]
pub struct SheetRows {
    /// The tracks the edit names.
    pub edited: Vec<CueTrack>,
    /// The artist every track of the sheet shows, not only the edited ones'.
    pub artists: Vec<(CueTrack, String)>,
}

/// The sheet at `path` with `edit` written for its tracks, or `None` where it already states all of
/// it. Blocking.
pub fn rewritten_sheet(
    path: &Path,
    edit: &SheetEdit,
    rows: &SheetRows,
) -> Result<Option<Vec<u8>>, AppError> {
    let sheet = EncodedSheet::read(path)?;
    let lines = sheet.lines();
    let mut rewrite = Rewrite::new(&lines, Files::Named);
    rewrite.apply(edit, rows);
    if !rewrite.changes() {
        return Ok(None);
    }
    Ok(Some(sheet.encode(&rewrite.into_lines(), path)))
}

/// A `CUESHEET` comment's rewrite, and the track fields the edit states.
pub(super) struct CommentRewrite {
    /// `None` where the comment already states all of the edit.
    pub(super) text: Option<String>,
    track_fields: Vec<(CueTrack, TrackField)>,
}

impl CommentRewrite {
    /// Whether the edit states `field` for its track, which an override of it would hide.
    pub(super) fn states(&self, field: (CueTrack, TrackField)) -> bool {
        self.track_fields.contains(&field)
    }
}

/// The `CUESHEET` comment `text` of the image at `origin`, rewritten for `edit` as a `.cue` is.
pub(super) fn rewritten_comment(
    text: &str,
    origin: &Path,
    edit: &SheetEdit,
    rows: &SheetRows,
) -> CommentRewrite {
    let sheet = EncodedSheet::from_text(text);
    let lines = sheet.lines();
    let mut rewrite = Rewrite::new(&lines, Files::Inside);
    rewrite.apply(edit, rows);
    let track_fields = std::mem::take(&mut rewrite.track_fields);
    let text = rewrite.changes().then(|| {
        String::from_utf8_lossy(&sheet.encode(&rewrite.into_lines(), origin)).into_owned()
    });
    CommentRewrite { text, track_fields }
}

/// Replaces the sheet at `path` with `bytes` through a copy renamed over it, so nothing reads half
/// a sheet and the file keeps its permissions. Blocking.
pub fn replace_sheet(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    atomic_file::rewrite_with_sync(path, |copy| {
        std::fs::write(copy, bytes)
            .map_err(|e| AppError::metadata(format!("Failed to write {}", path.display()), e))
    })
}

/// A command the writer states.
#[derive(Clone, Copy)]
enum Keyword {
    Title,
    Performer,
    Songwriter,
    RemComposer,
    Isrc,
    Catalog,
    Genre,
    Date,
    DiscNumber,
    TotalDiscs,
}

impl Keyword {
    /// The command a line of this keyword opens with, a `REM` line's key following it.
    fn word(self) -> &'static str {
        match self {
            Self::Title => "TITLE",
            Self::Performer => "PERFORMER",
            Self::Songwriter => "SONGWRITER",
            Self::Isrc => "ISRC",
            Self::Catalog => "CATALOG",
            Self::RemComposer | Self::Genre | Self::Date | Self::DiscNumber | Self::TotalDiscs => {
                "REM"
            }
        }
    }

    fn rem_key(self) -> Option<&'static str> {
        match self {
            Self::RemComposer => Some(rem::COMPOSER),
            Self::Genre => Some(rem::GENRE),
            Self::Date => Some(rem::DATE),
            Self::DiscNumber => Some(rem::DISCNUMBER),
            Self::TotalDiscs => Some(rem::TOTALDISCS),
            Self::Title | Self::Performer | Self::Songwriter | Self::Isrc | Self::Catalog => None,
        }
    }

    /// The track field a line of this keyword states, as an override would.
    fn track_field(self) -> Option<TrackField> {
        match self {
            Self::Title => Some(TrackField::Title),
            Self::Performer => Some(TrackField::Artist),
            Self::Songwriter | Self::RemComposer => Some(TrackField::Composer),
            Self::Isrc => Some(TrackField::Isrc),
            Self::Genre => Some(TrackField::Genre),
            Self::Date => Some(TrackField::Date),
            Self::Catalog | Self::DiscNumber | Self::TotalDiscs => None,
        }
    }

    fn states(self, command: &Command) -> bool {
        match command {
            Command::Title => matches!(self, Self::Title),
            Command::Performer => matches!(self, Self::Performer),
            Command::Songwriter => matches!(self, Self::Songwriter),
            Command::Isrc => matches!(self, Self::Isrc),
            Command::Catalog => matches!(self, Self::Catalog),
            Command::Rem(key) => self.rem_key() == Some(key.as_str()),
            Command::File | Command::Track | Command::Other => false,
        }
    }

    /// The line stating `value`, free text quoted and a number or a code bare, as rippers write
    /// them. The reader takes a quoted value up to the line's last quote, so an inner one is safe.
    fn spell(self, indent: &str, value: &str) -> String {
        let word = match self.rem_key() {
            Some(key) => format!("{} {key}", self.word()),
            None => self.word().to_owned(),
        };
        match self {
            Self::Title | Self::Performer | Self::Songwriter | Self::RemComposer | Self::Genre => {
                format!("{indent}{word} \"{value}\"")
            }
            Self::Isrc | Self::Catalog | Self::Date | Self::DiscNumber | Self::TotalDiscs => {
                format!("{indent}{word} {value}")
            }
        }
    }
}

#[derive(Clone)]
enum LineEdit {
    Keep,
    Drop,
    Replace(String),
}

/// One sheet's lines and what the edit does to each.
struct Rewrite<'a> {
    lines: &'a [RawLine<'a>],
    statements: Vec<Statement>,
    stated: Sheet,
    edits: Vec<LineEdit>,
    /// New lines, each before the line at its position, in the order they were written.
    inserts: Vec<(usize, String)>,
    /// What a new line ends with: the sheet's own ending.
    ending: LineEnding,
    /// Each track field the edit sets or clears, whether or not a line had to change for it.
    track_fields: Vec<(CueTrack, TrackField)>,
}

impl<'a> Rewrite<'a> {
    fn new(lines: &'a [RawLine<'a>], files: Files) -> Self {
        let (stated, statements) = statements(lines.iter().map(|line| line.text.as_str()), files);
        let ending = lines
            .iter()
            .map(|line| line.ending)
            .find(|&ending| ending != LineEnding::None)
            .unwrap_or(LineEnding::Lf);
        Self {
            lines,
            statements,
            stated,
            edits: vec![LineEdit::Keep; lines.len()],
            inserts: Vec::new(),
            ending,
            track_fields: Vec::new(),
        }
    }

    fn apply(&mut self, edit: &SheetEdit, rows: &SheetRows) {
        let tracks = rows.edited.as_slice();
        self.write(Scope::Sheet, Keyword::Title, &edit.album);
        self.pin_inherited_artists(edit, rows);
        self.write(Scope::Sheet, Keyword::Performer, &edit.album_artist);
        self.write(Scope::Sheet, Keyword::DiscNumber, &edit.disc_number);
        self.write(Scope::Sheet, Keyword::TotalDiscs, &edit.disc_total);
        self.write(Scope::Sheet, Keyword::Catalog, &edit.barcode);

        // The hidden track has no lines of its own, and a track the sheet no longer numbers none.
        let scopes: Vec<Scope> = tracks
            .iter()
            .filter(|&&track| self.track_line(track).is_some())
            .map(|&track| Scope::Track(track))
            .collect();
        for &scope in &scopes {
            self.write(scope, Keyword::Title, &edit.title);
            self.write(scope, Keyword::Performer, &edit.artist);
            self.write(scope, Keyword::Isrc, &edit.isrc);
        }
        self.write_shared_fields(edit, tracks, &scopes);
    }

    /// Gives each placed track stating no `PERFORMER` the artist it shows on a line of its own,
    /// where the edit moves the sheet's `PERFORMER`: such a track reads that line as its artist, or
    /// the image's artist where the sheet states none. A track whose artist the edit changes is
    /// left to it.
    fn pin_inherited_artists(&mut self, edit: &SheetEdit, rows: &SheetRows) {
        let album_artist = match &edit.album_artist {
            FieldEdit::Keep => return,
            FieldEdit::Clear => None,
            FieldEdit::Set(album_artist) => Some(one_line(album_artist)),
        };
        let album_artist = album_artist.filter(|album_artist| !album_artist.is_empty());
        if album_artist == self.stated.fields.performer {
            return;
        }
        let edited =
            |number: &CueTrack| edit.artist != FieldEdit::Keep && rows.edited.contains(number);
        let inheriting: Vec<CueTrack> = self
            .stated
            .tracks
            .iter()
            .filter(|track| is_placed(track) && track.fields.performer.is_none())
            .map(|track| track.number)
            .filter(|number| !edited(number))
            .collect();
        for number in inheriting {
            if let Some((_, artist)) = rows.artists.iter().find(|(track, _)| *track == number) {
                self.set(Scope::Track(number), Keyword::Performer, &one_line(artist));
            }
        }
    }

    /// Composer, genre and date, which a sheet can state for every track at once: written there,
    /// the tracks' own lines dropped, when the edit covers every track, and on each one otherwise.
    fn write_shared_fields(&mut self, edit: &SheetEdit, tracks: &[CueTrack], scopes: &[Scope]) {
        if !self.covers_every_track(tracks) {
            for &scope in scopes {
                self.write_shared(scope, edit);
            }
            return;
        }
        self.write_shared(Scope::Sheet, edit);
        let overridden = SheetEdit {
            composer: cleared(&edit.composer),
            genre: cleared(&edit.genre),
            year: cleared(&edit.year),
            ..SheetEdit::default()
        };
        for scope in self.track_scopes() {
            self.write_shared(scope, &overridden);
        }
    }

    fn write_shared(&mut self, scope: Scope, edit: &SheetEdit) {
        self.write(scope, Keyword::Songwriter, &edit.composer);
        // Read where `SONGWRITER` is missing, so it would come back once that is cleared.
        if edit.composer == FieldEdit::Clear {
            self.write(scope, Keyword::RemComposer, &FieldEdit::Clear);
        }
        self.write(scope, Keyword::Genre, &edit.genre);
        let date = self.dated(scope, &edit.year);
        self.write(scope, Keyword::Date, &date);
    }

    fn write(&mut self, scope: Scope, keyword: Keyword, edit: &FieldEdit<String>) {
        match edit {
            FieldEdit::Keep => {}
            FieldEdit::Clear => self.clear(scope, keyword),
            FieldEdit::Set(value) => self.set(scope, keyword, &one_line(value)),
        }
    }

    fn clear(&mut self, scope: Scope, keyword: Keyword) {
        self.note_stated(scope, keyword);
        for index in self.lines_stating(scope, keyword) {
            self.edits[index] = LineEdit::Drop;
        }
    }

    /// Writes `value` over the scope's first line of `keyword` and drops the others, so the one
    /// line left reads back whichever of them the reader would have taken.
    fn set(&mut self, scope: Scope, keyword: Keyword, value: &str) {
        self.note_stated(scope, keyword);
        if value.is_empty() {
            return self.clear(scope, keyword);
        }
        let stating = self.lines_stating(scope, keyword);
        let Some((&first, others)) = stating.split_first() else {
            return self.insert(scope, keyword, value);
        };
        let already_stated = stated_value(&self.lines[first].text).as_deref() == Some(value);
        if already_stated && others.is_empty() {
            return;
        }
        let indent = indent_of(&self.lines[first].text);
        self.edits[first] = LineEdit::Replace(keyword.spell(indent, value));
        for &index in others {
            self.edits[index] = LineEdit::Drop;
        }
    }

    fn note_stated(&mut self, scope: Scope, keyword: Keyword) {
        if let (Scope::Track(track), Some(field)) = (scope, keyword.track_field()) {
            self.track_fields.push((track, field));
        }
    }

    /// A track's new line goes straight under its `TRACK` line, ahead of the `INDEX` lines the
    /// format wants it before, and a sheet's ahead of the first `FILE`.
    fn insert(&mut self, scope: Scope, keyword: Keyword, value: &str) {
        let (at, indent) = match scope {
            Scope::Track(track) => {
                let Some(track_line) = self.track_line(track) else {
                    return;
                };
                (track_line + 1, self.track_indent(track_line))
            }
            Scope::Sheet => {
                let first = |command: Command| {
                    self.statements.iter().position(|statement| statement.command == command)
                };
                let at = first(Command::File).or_else(|| first(Command::Track));
                let at = at.unwrap_or(self.lines.len());
                let indent = self.lines.get(at).map_or("", |line| indent_of(&line.text));
                (at, indent.to_owned())
            }
            Scope::Rejected => return,
        };
        self.inserts.push((at, keyword.spell(&indent, value)));
    }

    /// The indentation of the line under a `TRACK` line, or the `TRACK` line's own one step in.
    fn track_indent(&self, track_line: usize) -> String {
        let scope = &self.statements[track_line].scope;
        match self.lines.get(track_line + 1) {
            Some(next) if self.statements[track_line + 1].scope == *scope => {
                indent_of(&next.text).to_owned()
            }
            _ => format!("{}  ", indent_of(&self.lines[track_line].text)),
        }
    }

    fn lines_stating(&self, scope: Scope, keyword: Keyword) -> Vec<usize> {
        let stating =
            |statement: &Statement| statement.scope == scope && keyword.states(&statement.command);
        let lines = self.statements.iter().enumerate();
        lines.filter(|(_, statement)| stating(statement)).map(|(index, _)| index).collect()
    }

    fn track_line(&self, track: CueTrack) -> Option<usize> {
        self.statements.iter().position(|statement| {
            statement.command == Command::Track && statement.scope == Scope::Track(track)
        })
    }

    fn placed_tracks(&self) -> impl Iterator<Item = CueTrack> {
        self.stated.tracks.iter().filter(|track| is_placed(track)).map(|track| track.number)
    }

    fn track_scopes(&self) -> Vec<Scope> {
        self.placed_tracks().map(Scope::Track).collect()
    }

    fn covers_every_track(&self, tracks: &[CueTrack]) -> bool {
        self.placed_tracks().all(|track| tracks.contains(&track))
    }

    /// `year` as the date written in `scope`, keeping the month and day the scope reads now.
    fn dated(&self, scope: Scope, year: &FieldEdit<u16>) -> FieldEdit<String> {
        let sheet_date = self.stated.fields.rem(rem::DATE);
        let date = match scope {
            Scope::Track(number) => {
                let track = self.stated.track(number);
                track.and_then(|track| track.fields.rem(rem::DATE)).or(sheet_date)
            }
            Scope::Sheet | Scope::Rejected => sheet_date,
        };
        let seed = date.and_then(|date| date.parse::<Timestamp>().ok()).unwrap_or_default();
        mapped(year, |&year| Some(Timestamp { year, ..seed }.to_string()))
    }

    fn changes(&self) -> bool {
        !self.inserts.is_empty() || self.edits.iter().any(|edit| !matches!(edit, LineEdit::Keep))
    }

    fn into_lines(mut self) -> Vec<OutLine<'a>> {
        self.inserts.sort_by_key(|(at, _)| *at);
        let mut inserts = self.inserts.into_iter().peekable();
        let mut out = Vec::with_capacity(self.lines.len() + inserts.len());
        for (index, (line, edit)) in self.lines.iter().zip(self.edits).enumerate() {
            while let Some((_, text)) = inserts.next_if(|(at, _)| *at == index) {
                out.push(OutLine::New(text, self.ending));
            }
            match edit {
                LineEdit::Keep => out.push(OutLine::Kept(line)),
                LineEdit::Drop => {}
                LineEdit::Replace(text) => out.push(OutLine::New(text, line.ending)),
            }
        }
        out.extend(inserts.map(|(_, text)| OutLine::New(text, self.ending)));
        out
    }
}

/// `edit` as a clear wherever it changes the field.
fn cleared<T>(edit: &FieldEdit<T>) -> FieldEdit<T> {
    match edit {
        FieldEdit::Keep => FieldEdit::Keep,
        FieldEdit::Clear | FieldEdit::Set(_) => FieldEdit::Clear,
    }
}

/// An audio track with an `INDEX 01`, the only kind a row stands for.
fn is_placed(track: &SheetTrack) -> bool {
    track.is_audio && track.index(1).is_some()
}

/// `value` on one line, a line break in it ending the line it is written on.
fn one_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_owned()
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}
