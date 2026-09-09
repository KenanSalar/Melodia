//! The one thing this module decides before it opens a socket.
//!
//! The directory names a recording by artist, title and duration together, so an artist tag that
//! is present and empty is a guaranteed miss. Read as filled, every untagged track in a library
//! spends a request on its way to being told nothing, at an interval the service asks us to keep.

use super::*;

#[test]
fn an_absent_artist_tag_is_not_a_field_to_search_by() {
    assert_eq!(filled(None), None);
}

#[test]
fn an_artist_tag_holding_only_whitespace_is_not_one_either() {
    // What a tagger leaves behind when they clear a field rather than remove it, and it reads as
    // present to every `is_some` in the tree.
    assert_eq!(filled(Some("")), None);
    assert_eq!(filled(Some("   \t ")), None);
}

#[test]
fn a_real_artist_tag_comes_back_trimmed() {
    assert_eq!(filled(Some("  Band  ")), Some("Band"));
}
