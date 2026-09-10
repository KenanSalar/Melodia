//! Which of the four sources answers, and which of two sheets wins once two of them have.
//!
//! `resolve` itself needs an `AppState`, which opens the audio card, so what is driven here is the
//! three functions it is made of. Between them they carry every decision it makes except the
//! online switch, which `crates/melodia/tests/lyrics_switch.rs` pins from the other side.
//!
//! Getting the order wrong is invisible: the panel draws a sheet either way, and the one it draws
//! is the one the user did not put there.

use tempfile::TempDir;

use super::*;
use melodia_core::entities::lyrics::LyricsSource;
use melodia_store::media::ingest::tag_writer;
use melodia_testkit::ASSETS_DIR;

/// A sheet the panel can follow, and one it cannot. The difference decides most of this module.
const TIMED: &str = "[00:01.00]a line with a stamp";
const PLAIN: &str = "a line with no stamp";

/// A track file with a lyrics tag this test can write, and a lyrics store beside it.
struct Staged {
    _tmp: TempDir,
    track: PathBuf,
    lyrics_dir: PathBuf,
}

impl Staged {
    fn new() -> Result<Self, AppError> {
        let tmp = TempDir::new()?;
        let track = tmp.path().join("track.mp3");
        std::fs::copy(PathBuf::from(ASSETS_DIR).join("silence.mp3"), &track)?;

        let lyrics_dir = tmp.path().join("lyrics");
        std::fs::create_dir_all(&lyrics_dir)?;
        Ok(Self { _tmp: tmp, track, lyrics_dir })
    }

    /// The path as the store keys it, which is the track's own path and nothing else.
    fn key(&self) -> String {
        self.track.to_string_lossy().into_owned()
    }

    fn write_tag(&self, text: &str) -> Result<(), AppError> {
        let edit = TagEdit { lyrics: FieldEdit::Set(text.to_owned()), ..TagEdit::default() };
        tag_writer::apply_to_file(&self.track, &edit, None)?;
        Ok(())
    }

    fn write_sidecar(&self, text: &str) -> Result<(), AppError> {
        std::fs::write(self.track.with_extension("lrc"), text)?;
        Ok(())
    }

    fn write_store(&self, text: &str) -> Result<(), AppError> {
        store::write(&self.lyrics_dir, &self.key(), store::Fetched::Sheet(text))
    }

    fn local(&self) -> Result<Local, AppError> {
        read_local(&self.track, &self.lyrics_dir, &self.key())
    }

    fn text(&self) -> Result<Option<String>, AppError> {
        read_local_text(&self.track, &self.lyrics_dir, &self.key())
    }
}

fn parsed(text: &str) -> Option<Lyrics> {
    lrc::parse(text, LyricsSource::Tag)
}

/// [`parsed`] where the case is about what wins rather than about the parse, so a parser that
/// stopped reading the fixture fails loudly instead of leaving the comparison unrun.
fn fixture(text: &str) -> Result<Lyrics, AppError> {
    parsed(text).ok_or_else(|| AppError::Validation("the fixture must parse".into()))
}

/// The first line of whichever sheet came back, which is what the cases tell two apart by.
fn first_line(outcome: &LyricsOutcome) -> Option<&str> {
    match outcome {
        LyricsOutcome::Sheet(sheet) => sheet.lines.first().map(|line| line.text.as_str()),
        _ => None,
    }
}

// --- `timed_first`: which of two sheets the panel is given ---

#[test]
fn with_nothing_of_our_own_the_directory_answer_stands() {
    // Including the ones that are not sheets: a track the directory calls instrumental has no tag
    // to fall back to, and saying so is the answer.
    assert_eq!(timed_first(LyricsOutcome::Absent, None), LyricsOutcome::Absent);
    assert_eq!(timed_first(LyricsOutcome::Instrumental, None), LyricsOutcome::Instrumental);
}

#[test]
fn a_timed_answer_beats_the_files_own_plain_tag() -> Result<(), AppError> {
    // The feature is the follow rather than the words, and a tagger who pasted prose into the tag
    // has expressed no preference against the sung line being marked.
    let won = timed_first(LyricsOutcome::Sheet(fixture(TIMED)?), parsed(PLAIN));

    assert_eq!(first_line(&won), Some("a line with a stamp"));
    Ok(())
}

#[test]
fn a_plain_answer_loses_to_the_files_own_tag() -> Result<(), AppError> {
    // Neither can be followed, so the tie goes to the file's own words over a stranger's upload.
    let won = timed_first(LyricsOutcome::Sheet(fixture("somebody elses words")?), parsed(PLAIN));

    assert_eq!(first_line(&won), Some("a line with no stamp"));
    Ok(())
}

#[test]
fn a_verdict_of_instrumental_loses_to_a_tag_with_words_in_it() {
    // The directory is answering about a recording it matched off our tags, and the file in hand
    // plainly has words. Not a special case: it is the same rule one arm down.
    let won = timed_first(LyricsOutcome::Instrumental, parsed(PLAIN));

    assert_eq!(first_line(&won), Some("a line with no stamp"));
}

// --- `read_local`: which local source answers ---

#[test]
fn a_sidecar_answers_and_the_store_is_never_consulted() -> Result<(), AppError> {
    // The file a user put there on purpose ends the question, which is what makes a wrong sheet
    // correctable without a setting.
    let staged = Staged::new()?;
    staged.write_tag(TIMED)?;
    staged.write_store(TIMED)?;
    staged.write_sidecar(PLAIN)?;

    let local = staged.local()?;

    assert!(local.is_sidecar);
    assert_eq!(
        local.own.as_ref().and_then(|s| s.lines.first()).map(|l| l.text.as_str()),
        Some("a line with no stamp")
    );
    assert!(local.stored.is_none(), "a sidecar answer must not cost a store read");
    Ok(())
}

#[test]
fn a_timed_tag_suppresses_the_store_read() -> Result<(), AppError> {
    // Nothing the store holds could better it, and this runs on every track change.
    let staged = Staged::new()?;
    staged.write_tag(TIMED)?;
    staged.write_store(TIMED)?;

    let local = staged.local()?;

    assert!(!local.is_sidecar);
    assert_eq!(local.own.as_ref().map(Lyrics::is_synced), Some(true));
    assert!(local.stored.is_none());
    Ok(())
}

#[test]
fn a_plain_tag_leaves_the_store_free_to_answer() -> Result<(), AppError> {
    // Both come back, because which one wins is `timed_first`'s question and not this one's.
    let staged = Staged::new()?;
    staged.write_tag(PLAIN)?;
    staged.write_store(TIMED)?;

    let local = staged.local()?;

    assert_eq!(local.own.as_ref().map(Lyrics::is_synced), Some(false));
    assert!(local.stored.is_some());
    Ok(())
}

#[test]
fn nothing_anywhere_is_no_sheet_rather_than_an_error() -> Result<(), AppError> {
    let staged = Staged::new()?;

    let local = staged.local()?;

    assert!(local.own.is_none());
    assert!(local.stored.is_none());
    assert!(!local.is_sidecar);
    Ok(())
}

// --- `read_local_text`: what the tag editor is offered ---

#[test]
fn the_sidecar_text_is_what_the_editor_is_offered() -> Result<(), AppError> {
    let staged = Staged::new()?;
    staged.write_tag(TIMED)?;
    staged.write_sidecar(PLAIN)?;

    assert_eq!(staged.text()?.as_deref(), Some(PLAIN));
    Ok(())
}

#[test]
fn a_timed_tag_beats_a_stored_sheet() -> Result<(), AppError> {
    let staged = Staged::new()?;
    staged.write_tag(TIMED)?;
    staged.write_store("[00:02.00]the store's own")?;

    assert_eq!(staged.text()?.as_deref(), Some(TIMED));
    Ok(())
}

#[test]
fn a_timed_stored_sheet_beats_a_plain_tag() -> Result<(), AppError> {
    // The panel is showing the timed one, so this is what a promotion has to write. Handing over
    // the tag instead offers the user something other than what they are looking at.
    let staged = Staged::new()?;
    staged.write_tag(PLAIN)?;
    staged.write_store(TIMED)?;

    assert_eq!(staged.text()?.as_deref(), Some(TIMED));
    Ok(())
}

#[test]
fn with_neither_timed_the_files_own_tag_wins() -> Result<(), AppError> {
    let staged = Staged::new()?;
    staged.write_tag(PLAIN)?;
    staged.write_store("somebody elses words")?;

    assert_eq!(staged.text()?.as_deref(), Some(PLAIN));
    Ok(())
}

#[test]
fn a_tag_of_nothing_but_blank_stamps_is_offered_though_no_sheet_parses_from_it()
-> Result<(), AppError> {
    // The one place the two paths genuinely disagree. `lrc::is_timed` asks whether the text
    // carries a stamp, where `Lyrics::new` refuses a sheet with nothing to draw, so the editor is
    // offered text the panel shows nothing for. Recorded rather than argued for: the editor's job
    // is to hand back what is in the file.
    let staged = Staged::new()?;
    staged.write_tag("[00:01.00]")?;

    assert_eq!(staged.text()?.as_deref(), Some("[00:01.00]"), "the editor is offered it");
    assert!(staged.local()?.own.is_none(), "and the panel draws nothing");
    Ok(())
}
