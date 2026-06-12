//! Tests for the shared detail-view track matcher `track_matches` — the
//! case-insensitive substring match over title + artist + album that
//! every detail search bar walks per keystroke. The needle is always
//! pre-lowercased by the caller, so these tests pass lowercased needles.

use super::*;

/// Build a minimal `TrackListRow` with only the three matched fields set.
fn mk(title: &str, artist: Option<&str>, album: Option<&str>) -> RsTrackListRow {
    RsTrackListRow {
        id: 1,
        file_path: "/m/1.flac".to_owned(),
        file_name: "1.flac".to_owned(),
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        album_artist: None,
        album: album.map(str::to_owned),
        genre: None,
        track_number: None,
        disc_number: None,
        year: None,
        artwork_path: None,
        duration_ms: 0,
        is_favorite: false,
        album_id: None,
        artist_id: None,
        genre_id: None,
        date_added: "2026-01-01T00:00:00Z".to_owned(),
        sort_key: None,
    }
}

#[test]
fn matches_title_case_insensitively() {
    let row = mk("Bohemian Rhapsody", None, None);
    assert!(track_matches(&row, "rhap"));
    assert!(track_matches(&row, "bohemian"));
}

#[test]
fn matches_artist_substring() {
    let row = mk("Track", Some("Queen"), None);
    assert!(track_matches(&row, "quee"));
}

#[test]
fn matches_album_substring() {
    let row = mk("Track", None, Some("A Night at the Opera"));
    assert!(track_matches(&row, "opera"));
}

#[test]
fn no_match_returns_false() {
    let row = mk("Track", Some("Queen"), Some("Opera"));
    assert!(!track_matches(&row, "zzz"));
}

#[test]
fn absent_artist_and_album_never_match() {
    let row = mk("Track", None, None);
    assert!(!track_matches(&row, "queen"));
    assert!(track_matches(&row, "track"));
}

#[test]
fn empty_needle_matches_any_row() {
    // An empty string is a substring of every string. Callers
    // short-circuit empty needles before calling, but the matcher's own
    // contract still holds.
    let row = mk("Track", None, None);
    assert!(track_matches(&row, ""));
}

#[test]
fn non_ascii_falls_back_to_unicode_lowering() {
    // Exercises the allocating Unicode path (the zero-allocation byte
    // walk only handles all-ASCII haystack + needle).
    let row = mk("Über den Wolken", Some("Größenwahn"), None);
    assert!(track_matches(&row, "über"));
    assert!(track_matches(&row, "größe"));
    assert!(!track_matches(&row, "wölken"));
}

#[test]
fn ascii_needle_against_non_ascii_haystack_matches() {
    // Mixed case: haystack is non-ASCII, needle is ASCII — must still
    // route through the Unicode path and match the ASCII substring.
    let row = mk("Café del Mar", None, None);
    assert!(track_matches(&row, "del mar"));
}
