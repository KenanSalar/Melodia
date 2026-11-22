//! What the finder tells a person about a candidate. Wrong, Apply seems to do nothing, or a row is
//! badged for timings it doesn't carry.

use super::*;
use melodia_core::entities::lyrics::LyricsAnswer;

/// Every kind, in the badge's order, so a row of [`Outranking::hides`] answers reads across them.
const KINDS: [ChoiceKind; 4] =
    [ChoiceKind::Words, ChoiceKind::Lines, ChoiceKind::Plain, ChoiceKind::Instrumental];

fn hidden_by(outranking: Outranking) -> [bool; 4] {
    KINDS.map(|kind| outranking.hides(kind))
}

fn answer(synced: Option<&str>, plain: Option<&str>, instrumental: bool) -> LyricsAnswer {
    LyricsAnswer {
        synced: synced.map(str::to_owned),
        plain: plain.map(str::to_owned),
        instrumental,
    }
}

fn read(answer: LyricsAnswer) -> Option<Choice> {
    Choice::read(LyricsCandidate {
        title: "Song".to_owned(),
        artist: "Band".to_owned(),
        album: "Record".to_owned(),
        duration_secs: 180,
        answer,
    })
}

fn kind_of(answer: LyricsAnswer) -> Option<ChoiceKind> {
    read(answer).map(|choice| choice.kind)
}

#[test]
fn a_sidecar_hides_every_pick() {
    assert_eq!(hidden_by(Outranking::Sidecar), [true; 4]);
}

#[test]
fn a_timed_tag_hides_every_pick() {
    assert_eq!(hidden_by(Outranking::TimedTag), [true; 4]);
}

#[test]
fn a_plain_tag_hides_only_a_pick_that_cannot_be_followed() {
    // A timed pick outranks the file's own prose, as a timed lookup does.
    assert_eq!(hidden_by(Outranking::PlainTag), [false, false, true, true]);
}

#[test]
fn a_track_with_no_lyrics_of_its_own_hides_no_pick() {
    assert_eq!(hidden_by(Outranking::Nothing), [false; 4]);
}

#[test]
fn a_candidate_is_badged_by_the_finest_timing_its_sheet_carries() {
    let words = answer(Some("[00:01.00]<00:01.00>a <00:01.50>word"), None, false);
    let lines = answer(Some("[00:01.00]a line"), Some("a line"), false);
    let plain = answer(None, Some("a line"), false);

    assert_eq!(
        [words, lines, plain].map(kind_of),
        [Some(ChoiceKind::Words), Some(ChoiceKind::Lines), Some(ChoiceKind::Plain)]
    );
}

#[test]
fn an_instrumental_candidate_with_no_words_is_offered_as_instrumental_with_nothing_to_preview() {
    let instrumental = read(answer(None, None, true));

    assert_eq!(
        instrumental.map(|choice| (choice.kind, choice.preview)),
        Some((ChoiceKind::Instrumental, String::new()))
    );
}

#[test]
fn a_candidate_marked_instrumental_that_carries_words_is_offered_as_its_words() {
    // As the store keeps it, so the badge names what Apply stores.
    assert_eq!(kind_of(answer(Some("[00:01.00]a line"), None, true)), Some(ChoiceKind::Lines));
}

#[test]
fn a_candidate_with_no_words_and_no_instrumental_mark_is_left_out() {
    assert_eq!(kind_of(answer(None, Some("   "), false)), None);
}

#[test]
fn a_candidate_whose_sheet_draws_nothing_is_left_out() {
    assert_eq!(kind_of(answer(Some("[ti:Song]"), None, false)), None);
}

#[test]
fn the_preview_is_the_sheet_s_lines_without_stamps_or_glosses() {
    let glossed =
        answer(Some("[00:01.00]<00:01.00>one <00:01.50>two^uno dos\n[00:02.00]three"), None, false);

    assert_eq!(read(glossed).map(|choice| choice.preview), Some("one two\nthree".to_owned()));
}
