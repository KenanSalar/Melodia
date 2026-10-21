//! Tests for the shared row matcher. `track_matches` is the folded
//! substring match over the six fields every filter box searches. Needles
//! always arrive from `fold_needle`, so these tests fold theirs the same way
//! rather than hand-lowercasing. The needle's own predicates are
//! `melodia-core`'s `needle_tests`.

use super::*;
use melodia_core::utils::needle::fold_needle;

/// Build a `TrackListRow` with a distinct value in each searchable field,
/// so a test needle can only be answered by the field it names.
fn mk(
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
    genre: Option<&str>,
    year: Option<i32>,
) -> TrackListRow {
    TrackListRow {
        id: 1,
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        album_artist: None,
        album: album.map(str::to_owned),
        genre: genre.map(str::to_owned),
        track_number: None,
        disc_number: None,
        year,
        artwork_path: None,
        duration_ms: 0,
        is_favorite: false,
        rating: 0,
        album_id: None,
        artist_id: None,
        genre_id: None,
        sort_key: None,
    }
}

/// The three-field row the pre-widening tests used.
fn plain(title: &str, artist: Option<&str>, album: Option<&str>) -> TrackListRow {
    mk(title, artist, album, None, None)
}

#[test]
fn matches_title_case_insensitively() {
    let row = plain("Bohemian Rhapsody", None, None);
    assert!(track_matches(&row, &fold_needle("rhap")));
    assert!(track_matches(&row, &fold_needle("BOHEMIAN")));
}

#[test]
fn matches_artist_substring() {
    let row = plain("Track", Some("Queen"), None);
    assert!(track_matches(&row, &fold_needle("quee")));
}

#[test]
fn matches_album_substring() {
    let row = plain("Track", None, Some("A Night at the Opera"));
    assert!(track_matches(&row, &fold_needle("opera")));
}

#[test]
fn no_match_returns_false() {
    let row = plain("Track", Some("Queen"), Some("Opera"));
    assert!(!track_matches(&row, &fold_needle("zzz")));
}

#[test]
fn absent_fields_never_match() {
    let row = plain("Track", None, None);
    assert!(!track_matches(&row, &fold_needle("queen")));
    assert!(track_matches(&row, &fold_needle("track")));
}

#[test]
fn empty_needle_matches_any_row() {
    // An unfiltered list shows every row — every caller relies on this
    // rather than short-circuiting the empty needle itself.
    let row = plain("Track", None, None);
    assert!(track_matches(&row, &fold_needle("")));
}

// --- The fields the FTS index gained in 20260802000001 ---

#[test]
fn matches_genre() {
    let row = mk("Track", None, None, Some("Post-Rock"), None);
    assert!(track_matches(&row, &fold_needle("post-rock")));
    assert!(track_matches(&row, &fold_needle("rock")));
}

#[test]
fn matches_album_artist_when_it_differs_from_the_performer() {
    // The case album artist exists for: a compilation credited to one name
    // while every track names its own performer.
    let mut row = plain("Track", Some("Guest Vocalist"), Some("Compilation"));
    row.album_artist = Some("Various Artists".to_owned());
    assert!(track_matches(&row, &fold_needle("various")));
}

#[test]
fn matches_year_and_its_decade() {
    let row = mk("Track", None, None, None, Some(1998));
    assert!(track_matches(&row, &fold_needle("1998")));
    assert!(track_matches(&row, &fold_needle("199")));
    // Substring, not prefix — every other field in these boxes matches
    // mid-word, so the year does too.
    assert!(track_matches(&row, &fold_needle("98")));
    assert!(!track_matches(&row, &fold_needle("1997")));
}

#[test]
fn an_absent_year_never_matches_a_digit_needle() {
    let row = mk("Track", None, None, None, None);
    assert!(!track_matches(&row, &fold_needle("1998")));
}

#[test]
fn a_digit_needle_still_reaches_the_text_fields() {
    // The year guard decides whether to format a year, not whether to look
    // at anything else — a title of digits must still match.
    let row = mk("1999", None, None, None, Some(1982));
    assert!(track_matches(&row, &fold_needle("1999")));
}

// --- Accent folding, the other half of what the Search view gained ---

#[test]
fn an_ascii_needle_reaches_an_accented_field() {
    let row = plain("Jóga", Some("Björk"), None);
    assert!(track_matches(&row, &fold_needle("bjork")));
    assert!(track_matches(&row, &fold_needle("joga")));
}

#[test]
fn an_accented_needle_reaches_an_ascii_field() {
    // The fold is symmetric — both sides go through it, so neither
    // spelling is privileged.
    let row = plain("Joga", Some("Bjork"), None);
    assert!(track_matches(&row, &fold_needle("Björk")));
}

#[test]
fn an_ascii_needle_reaches_a_two_mark_character() {
    // SQLite's legacy `remove_diacritics 1` folds one combining mark and
    // stops; mode 2 — which `push_folded` mirrors — reaches scripts like
    // Vietnamese, where `ế` carries two. Same case the FTS test pins.
    let row = plain("Bế Tắc", None, None);
    assert!(track_matches(&row, &fold_needle("be")));
    assert!(track_matches(&row, &fold_needle("tac")));
}

#[test]
fn folding_does_not_collapse_distinct_letters() {
    let row = plain("Über den Wolken", Some("Größenwahn"), None);
    assert!(track_matches(&row, &fold_needle("uber")));
    assert!(!track_matches(&row, &fold_needle("wolkon")));
}

#[test]
fn fold_needle_trims() {
    // Six call sites used to lowercase without trimming, so a trailing
    // space silently emptied those lists.
    let row = plain("Bohemian Rhapsody", None, None);
    assert!(track_matches(&row, &fold_needle("  rhap  ")));
    assert!(track_matches(&row, &fold_needle("   ")));
}

#[test]
fn a_nul_in_a_field_matches_the_way_the_packed_key_does() {
    // ID3v2.4 joins a multi-value text frame with `\0`, so this is a real
    // tag rather than a hypothetical. The packed `search_text` maps it to a
    // space (`SearchFields::pack`); `Needle::contains` has to agree, on both
    // its arms — the ASCII byte walk
    // used to skip the mapping, so one non-ASCII character anywhere in the
    // field flipped the answer.
    let row = plain("Track", Some("Queen\0David Bowie"), None);
    assert!(track_matches(&row, &fold_needle("queen david")));

    let accented = plain("Track", Some("Queen\0Davíd Bowie"), None);
    assert!(track_matches(&accented, &fold_needle("queen david")));
}
