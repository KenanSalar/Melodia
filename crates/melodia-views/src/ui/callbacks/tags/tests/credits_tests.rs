//! The join-phrase picker, and the field discriminator the two credits share.

use super::super::form::{CREDIT_ALBUM_ARTIST, CREDIT_ARTIST};
use super::{credit_field, phrase_at, register_phrase};
use melodia_core::entities::artist::JOIN_PHRASES;

fn builtin() -> Vec<(String, String)> {
    JOIN_PHRASES
        .into_iter()
        .map(|(label, rendered)| (label.to_owned(), rendered.to_owned()))
        .collect()
}

/// **Round-tripping somebody else's " meets " matters more than a tidy picker.** Without the
/// append, opening such a file and saving any field would silently rewrite the credit to whatever
/// sits at index 0.
#[test]
fn a_phrase_the_picker_has_never_heard_of_is_appended_rather_than_lost() {
    let mut phrases = builtin();
    let before = phrases.len();

    let index = register_phrase(&mut phrases, " meets ");

    assert_eq!(usize::try_from(index), Ok(before));
    assert_eq!(phrase_at(&phrases, index), " meets ");
    // The label is the trimmed form, which is what a picker row reads.
    assert_eq!(phrases.get(before).map(|(label, _)| label.as_str()), Some("meets"));
}

#[test]
fn a_phrase_the_picker_already_offers_reuses_its_own_row() {
    let mut phrases = builtin();
    let before = phrases.len();

    let index = register_phrase(&mut phrases, " feat. ");

    assert_eq!(index, 0, "the first entry of the built-in list");
    assert_eq!(phrases.len(), before, "nothing was appended");
}

/// The last credit in a line has no phrase after it, and index 0 is where that lands — so an empty
/// phrase must not append a row of its own.
#[test]
fn the_last_credits_empty_phrase_registers_nothing() {
    let mut phrases = builtin();
    let before = phrases.len();

    assert_eq!(register_phrase(&mut phrases, ""), 0);
    assert_eq!(phrases.len(), before);
}

#[test]
fn a_second_unknown_phrase_gets_its_own_row() {
    let mut phrases = builtin();
    let before = phrases.len();

    let first = register_phrase(&mut phrases, " meets ");
    let second = register_phrase(&mut phrases, " versus ");

    assert_ne!(first, second);
    assert_eq!(phrases.len(), before + 2);
    assert_eq!(phrase_at(&phrases, second), " versus ");
}

/// An index the list cannot answer is no phrase at all, which is what the last credit in a line
/// wants anyway — a stale index must not reach into somebody else's row.
#[test]
fn an_index_the_list_cannot_answer_renders_no_phrase() {
    let phrases = builtin();

    assert_eq!(phrase_at(&phrases, -1), "");
    assert_eq!(phrase_at(&phrases, i32::MAX), "");
    assert_eq!(phrase_at(&[], 0), "");
}

/// Slint hands the discriminator over as a bare `int`, so anything but the album artist is the
/// track artist — the direction that fails safe, a wrong answer here writing one credit's rows
/// into the other.
#[test]
fn any_field_but_the_album_artist_is_the_track_artist() {
    assert_eq!(credit_field(1), CREDIT_ALBUM_ARTIST);
    assert_eq!(credit_field(0), CREDIT_ARTIST);
    assert_eq!(credit_field(-1), CREDIT_ARTIST);
    assert_eq!(credit_field(99), CREDIT_ARTIST);
}
