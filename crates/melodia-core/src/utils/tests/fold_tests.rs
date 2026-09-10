//! What the fold answers at its own edges.
//!
//! Two crates compare strings through this and neither can see the other's cases, so the
//! properties the two paths share are pinned here rather than in whichever consumer noticed first.

use super::*;

#[test]
fn both_paths_answer_a_nul_with_a_space() {
    // The byte walk and the decomposing walk share nothing but this rule, so it is the one an edit
    // to either can break in half. ID3v2.4 joins a multi-value frame with a NUL, so the answer
    // decides whether two spellings of one credit compare equal.
    assert_eq!(fold("a\0b"), "a b", "the ascii walk");
    assert_eq!(fold("\u{e1}\0b"), "a b", "the decomposing walk");
}

#[test]
fn a_nul_is_the_only_ascii_byte_the_fold_moves() {
    assert_eq!(fold_ascii_byte(0), b' ');
    assert_eq!(fold_ascii_byte(b'A'), b'a');
    assert_eq!(fold_ascii_byte(b'z'), b'z');
    assert_eq!(fold_ascii_byte(b'7'), b'7');
    assert_eq!(fold_ascii_byte(b'-'), b'-');
}

#[test]
fn the_two_spellings_of_an_accented_letter_fold_alike() {
    // Precomposed against decomposed. A tagger's choice between them is invisible on screen and is
    // the whole reason the comparison normalizes at all.
    assert_eq!(fold("\u{e9}"), fold("e\u{301}"));
    assert_eq!(fold("\u{e9}"), "e");
}

#[test]
fn a_mark_outside_latin_comes_off_too() {
    // `is_combining_mark` is the Mark category rather than a Latin table, which is what reaches
    // these two. Under-folding is the half that would show, a filter silently matching less.
    assert_eq!(fold("\u{304c}"), "\u{304b}", "a kana voicing mark");
    assert_eq!(fold("\u{915}\u{940}"), "\u{915}", "an indic spacing mark");
}

#[test]
fn a_dotted_capital_i_folds_to_a_plain_one() {
    // It decomposes to `I` plus the dot, so the mark drop is what lands it on `i` rather than on a
    // letter no needle carries. Turkish titles reach the lyrics matcher through this.
    assert_eq!(fold("\u{130}stanbul"), "istanbul");
}

#[test]
fn a_string_of_nothing_but_marks_folds_to_nothing() {
    assert_eq!(fold("\u{301}\u{300}"), "");
}

#[test]
fn an_empty_string_folds_to_an_empty_string() {
    assert_eq!(fold(""), "");
}

#[test]
fn push_folded_appends_rather_than_replaces() {
    // The row matcher packs several fields into one buffer, so a write that cleared it first would
    // leave every field but the last unsearchable.
    let mut out = String::from("kept ");
    push_folded(&mut out, "ADDED");
    assert_eq!(out, "kept added");
}
