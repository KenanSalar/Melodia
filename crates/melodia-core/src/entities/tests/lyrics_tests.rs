//! The two invariants the vocabulary carries, and the preference the directory's answer states.

use super::*;

fn line(at_ms: Option<i64>, text: &str) -> LyricLine {
    LyricLine { at_ms, end_ms: None, text: text.to_owned(), romanization: None, translation: None }
}

#[test]
fn a_sheet_with_no_lines_is_no_sheet() {
    assert_eq!(Lyrics::new(Vec::new(), LyricsSource::Tag), None);
}

#[test]
fn a_sheet_of_nothing_but_blanks_is_no_sheet() {
    // A lyrics tag holding only newlines is one a tagger created and nobody filled in; drawing it
    // hands the reader a bare panel that claims to have found something.
    let blanks = vec![line(None, ""), line(None, "   \t ")];
    assert_eq!(Lyrics::new(blanks, LyricsSource::Tag), None);
}

#[test]
fn blank_lines_inside_a_sheet_are_kept() {
    // How a plain sheet spaces its verses.
    let verses = vec![line(None, "first"), line(None, ""), line(None, "second")];
    let sheet = Lyrics::new(verses, LyricsSource::Sidecar);
    assert_eq!(sheet.map(|s| s.lines.len()), Some(3));
}

#[test]
fn a_sheet_carrying_a_stamp_can_be_followed() {
    let timed = Lyrics::new(vec![line(Some(1_000), "a")], LyricsSource::Online);
    assert_eq!(timed.map(|s| s.is_synced()), Some(true));
}

#[test]
fn a_sheet_carrying_no_stamp_cannot() {
    let plain = Lyrics::new(vec![line(None, "a")], LyricsSource::Online);
    assert_eq!(plain.map(|s| s.is_synced()), Some(false));
}

#[test]
fn a_directory_answer_prefers_the_timed_sheet() {
    let answer = LyricsAnswer {
        synced: Some("[00:01.00]timed".to_owned()),
        plain: Some("untimed".to_owned()),
        instrumental: false,
    };
    assert_eq!(answer.text(), Some("[00:01.00]timed"));
}

#[test]
fn an_empty_timed_field_does_not_shadow_a_real_plain_one() {
    // Each field is judged before it is preferred. Picking the timed one and testing it afterwards
    // threw away perfectly good plain lyrics whenever the directory answered with `""`.
    let answer = LyricsAnswer {
        synced: Some("   \n ".to_owned()),
        plain: Some("untimed".to_owned()),
        instrumental: false,
    };
    assert_eq!(answer.text(), Some("untimed"));
}

#[test]
fn an_answer_with_neither_field_filled_has_no_text() {
    let answer = LyricsAnswer { synced: None, plain: None, instrumental: false };
    assert_eq!(answer.text(), None);
}

#[test]
fn an_instrumental_carries_no_text_of_its_own() {
    // The flag is the answer; there is nothing to draw and nothing to store as a sheet.
    let answer = LyricsAnswer { synced: None, plain: None, instrumental: true };
    assert_eq!(answer.text(), None);
    assert!(answer.instrumental);
}

#[test]
fn a_blank_timed_field_does_not_call_an_answer_timed() {
    // This is the branch a lookup decides a second request on, and it has to read the field the
    // way `text` will: tested for presence alone, a `""` here ends the search on an answer that
    // then hands over nothing.
    let answer = LyricsAnswer { synced: Some("   ".to_owned()), plain: None, instrumental: false };
    assert!(!answer.is_synced());
}

#[test]
fn a_filled_timed_field_does() {
    let answer = LyricsAnswer {
        synced: Some("[00:01.00]timed".to_owned()),
        plain: None,
        instrumental: false,
    };
    assert!(answer.is_synced());
}
