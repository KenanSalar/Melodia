//! Tests for the shared row matchers. `track_matches` is the folded
//! substring match over the six fields every filter box searches;
//! `most_played_matches` is the card sibling running the same rule over a
//! narrower projection. Needles always arrive from `fold_needle`, so these
//! tests fold theirs the same way rather than hand-lowercasing.

use super::*;

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
        file_path: "/m/1.flac".to_owned(),
        file_name: "1.flac".to_owned(),
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
        date_added: "2026-01-01T00:00:00Z".to_owned(),
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
fn a_needle_with_a_non_digit_never_reaches_the_year() {
    // The guard that keeps the ordinary text query off the formatting path.
    // "199x" can't name a year, so a row whose only candidate is one misses.
    let row = mk("Track", None, None, None, Some(1998));
    assert!(!fold_needle("199x").matches_year(row.year));
    assert!(!fold_needle("").matches_year(row.year));
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
    // tag rather than a hypothetical. The Tracks view's packed key maps it
    // to a space (`tracks_tests::a_nul_in_a_field_cannot_forge_a_separator`);
    // `Needle::contains` has to agree, on both its arms — the ASCII byte walk
    // used to skip the mapping, so one non-ASCII character anywhere in the
    // field flipped the answer.
    let row = plain("Track", Some("Queen\0David Bowie"), None);
    assert!(track_matches(&row, &fold_needle("queen david")));

    let accented = plain("Track", Some("Queen\0Davíd Bowie"), None);
    assert!(track_matches(&accented, &fold_needle("queen david")));
}

#[test]
fn folding_is_idempotent() {
    // Views differ in whether they fold at write time or read time, and a
    // path that does both must not change the answer.
    let once = fold_needle("Björk");
    assert_eq!(fold_needle(once.as_str()), once);
}

/// Both shape flags are taken off the **folded** text, and nothing else here
/// can fail on it: the two arms of every predicate agree by construction, so
/// asking `raw` instead stays correct and quietly strands every accented needle
/// on the allocating one — which is the cost `Needle` exists to remove.
///
/// `Björk` is the case that shows it. It folds to `bjork`, so the ASCII byte
/// walk is reachable for it; the raw string is not ASCII and would never get
/// there.
#[test]
fn the_shape_flags_are_taken_after_the_fold() {
    assert!(fold_needle("Björk").ascii);
    assert!(fold_needle("  1998 ").digits);
    assert!(!fold_needle("199x").digits);
}

/// `Default` is hand-written so that it and `fold_needle("")` are the *same*
/// needle — a derived one leaves `ascii` false, which folding can never
/// produce. Behaviour is identical either way (an empty needle short-circuits
/// before the flag is read), so what the derive would actually cost is
/// `Needle::clear()`'s result taking the allocating arm of `equals` forever
/// after, and two needles that behave alike comparing unequal.
#[test]
fn the_default_needle_is_the_one_folding_an_empty_string_gives() {
    assert_eq!(Needle::default(), fold_needle(""));

    let mut cleared = fold_needle("Björk");
    cleared.clear();
    assert_eq!(cleared, fold_needle(""));
}

// --- most_played_matches ---
//
// The card predicate is load-bearing twice over on both Most Played tabs: the
// model build (`favorites::grids::apply::build_filtered_grids`,
// `recently_played::grid::apply::build_filtered_grid`) and the
// `most_played_track_ids` walk that resolves the ids `play-track` enqueues. A
// drift between the two hands `player_play_tracks` a list the cards aren't
// showing.

/// Build a `MostPlayedFavorite` with only the two rendered fields set.
fn mk_most_played(title: &str, artist: Option<&str>) -> MostPlayedFavorite {
    MostPlayedFavorite {
        id: 1,
        title: title.to_owned(),
        artist: artist.map(str::to_owned),
        album_artist: None,
        album: None,
        genre: None,
        year: None,
        artwork_path: None,
        play_count: 0,
        duration_ms: 0,
    }
}

#[test]
fn strip_matches_title_case_insensitively() {
    let card = mk_most_played("Under Pressure", None);
    assert!(most_played_matches(&card, &fold_needle("pressure")));
    assert!(most_played_matches(&card, &fold_needle("under")));
}

#[test]
fn strip_matches_artist_substring() {
    let card = mk_most_played("Track", Some("David Bowie"));
    assert!(most_played_matches(&card, &fold_needle("bowie")));
}

#[test]
fn strip_absent_artist_never_matches() {
    let card = mk_most_played("Track", None);
    assert!(!most_played_matches(&card, &fold_needle("bowie")));
    assert!(most_played_matches(&card, &fold_needle("track")));
}

#[test]
fn strip_empty_needle_matches_any_card() {
    // An unfiltered strip enqueues every card — both callers rely on this
    // rather than short-circuiting the empty needle themselves.
    let card = mk_most_played("Track", None);
    assert!(most_played_matches(&card, &fold_needle("")));
}

#[test]
fn a_card_and_a_track_row_answer_a_needle_the_same_way() {
    // On Recently Played the two surfaces share a scroll and a search bar, so
    // a narrower card projection is what let one query show a genre's tracks
    // in the list and none of its cards above them. Every field
    // `track_matches` reads has to be reachable from the card too.
    let mut card = mk_most_played("Ghost Town", Some("The Specials"));
    card.album = Some("More Specials".to_owned());
    card.album_artist = Some("Various Artists".to_owned());
    card.genre = Some("Ska".to_owned());
    card.year = Some(1981);

    let mut row = plain("Ghost Town", Some("The Specials"), Some("More Specials"));
    row.album_artist = Some("Various Artists".to_owned());
    row.genre = Some("Ska".to_owned());
    row.year = Some(1981);

    for needle in ["ghost", "specials", "various", "more", "ska", "1981", "zzz"] {
        let folded = fold_needle(needle);
        assert_eq!(
            most_played_matches(&card, &folded),
            track_matches(&row, &folded),
            "card and row disagree on {needle:?}"
        );
    }
}

#[test]
fn equals_and_starts_with_fold_case_and_accents() {
    let needle = fold_needle("  BJORK ");
    assert!(needle.equals("Björk"));
    assert!(needle.starts_with("Björk"));
    assert!(needle.starts_with("Björk Guðmundsdóttir"));

    // A prefix is not an equality, and a suffix is neither.
    assert!(!needle.equals("Björk Guðmundsdóttir"));
    assert!(!needle.starts_with("The Björk"));
}

/// The ASCII fast path and the folding path have to answer alike — the fast
/// path skips the allocation, not the rule.
#[test]
fn the_ascii_fast_path_agrees_with_the_folding_path() {
    const CASES: [(&str, &str); 6] = [
        ("Rock", "rock"),
        ("Röck", "rock"),
        ("rock", "Röck"),
        ("Rockabilly", "rock"),
        ("Röckabilly", "rock"),
        ("Pop", "rock"),
    ];
    for (haystack, raw) in CASES {
        let needle = fold_needle(raw);
        let folded_haystack: String = {
            let mut out = String::new();
            push_folded(&mut out, haystack);
            out
        };
        assert_eq!(
            needle.equals(haystack),
            folded_haystack == needle.as_str(),
            "Needle::equals disagrees with the fold on {haystack:?} / {raw:?}"
        );
        assert_eq!(
            needle.starts_with(haystack),
            folded_haystack.starts_with(needle.as_str()),
            "Needle::starts_with disagrees with the fold on {haystack:?} / {raw:?}"
        );
    }
}

/// The empty needle splits the two on purpose: "contains nothing" is what
/// lets a filter walk run with an empty search bar, but "equals nothing" is a
/// real question, and only an empty string answers it.
#[test]
fn the_empty_needle_means_different_things_to_the_three_predicates() {
    let empty = fold_needle("");
    assert!(empty.contains("Björk"));
    assert!(empty.starts_with("Björk"));
    assert!(!empty.equals("Björk"));
    assert!(empty.equals(""));
}
