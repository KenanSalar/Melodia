//! `RowSearchKey` — the Tracks view's packed filter key.
//!
//! It is the one matcher that doesn't call `row_match::track_matches`: the
//! list is the largest in the app, so its fields are folded once per fetch
//! and packed into a single string a keystroke can `contains` without
//! allocating. That makes it the one place the two answers to "does this
//! row match" could drift apart, which is what the last test here pins.

use super::*;

/// A row with a distinct value in every searchable field, so a needle can
/// only be answered by the field it names.
fn row(id: i64) -> RsTrackListRow {
    RsTrackListRow {
        id,
        file_path: "/m/1.flac".to_owned(),
        file_name: "1.flac".to_owned(),
        title: "Ghost Town".to_owned(),
        artist: Some("The Specials".to_owned()),
        album_artist: Some("Various Artists".to_owned()),
        album: Some("More Specials".to_owned()),
        genre: Some("Ska".to_owned()),
        track_number: None,
        disc_number: None,
        year: Some(1981),
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

#[test]
fn the_packed_key_matches_every_searchable_field() {
    let key = RowSearchKey::from_row(&row(1));
    for needle in ["ghost", "specials", "various", "more", "ska", "1981", "198"] {
        assert!(key.matches(&row_match::fold_needle(needle)), "missed {needle:?}");
    }
    assert!(!key.matches(&row_match::fold_needle("zzz")));
}

#[test]
fn an_empty_needle_matches_the_packed_key() {
    // The unfiltered list relies on this rather than branching itself.
    assert!(RowSearchKey::from_row(&row(1)).matches(&row_match::fold_needle("")));
}

#[test]
fn the_packed_key_folds_accents() {
    let mut r = row(1);
    r.artist = Some("Björk".to_owned());
    r.title = "Bế Tắc".to_owned();
    let key = RowSearchKey::from_row(&r);
    assert!(key.matches(&row_match::fold_needle("bjork")));
    assert!(key.matches(&row_match::fold_needle("be")));
}

#[test]
fn a_needle_cannot_straddle_the_packed_separator() {
    // Fields are joined by `\0`; a needle spanning two of them would make
    // the Tracks view match rows no other list does.
    let key = RowSearchKey::from_row(&row(1));
    assert!(!key.matches(&row_match::fold_needle("townthe")));
}

#[test]
fn a_nul_in_a_field_cannot_forge_a_separator() {
    // `push_folded` maps NUL to a space, so a tag carrying one can't split
    // its own field in two and let a needle match across the seam.
    let mut r = row(1);
    r.title = "Ghost\0Town".to_owned();
    let key = RowSearchKey::from_row(&r);
    assert!(key.matches(&row_match::fold_needle("ghost town")));
}

#[test]
fn the_packed_key_and_track_matches_agree_field_for_field() {
    // Structural drift is already hard — both read `row_match::search_fields`
    // — but the year is matched by a rule each calls separately, and a
    // disagreement would show as the Tracks view narrowing differently from
    // every detail page for the same query.
    //
    // The second row is the case that isn't structural at all: a field
    // carrying a `\0` (ID3v2.4's multi-value separator) is folded to a space
    // on the packed side, and `Needle::contains`' ASCII byte walk used to skip
    // that mapping — so the two agreed on every clean row and parted on the
    // one shape the packing exists to handle.
    let mut nul = row(2);
    nul.artist = Some("Queen\0David Bowie".to_owned());

    for r in [row(1), nul] {
        let key = RowSearchKey::from_row(&r);
        for needle in [
            "ghost", "specials", "various", "more", "ska", "1981", "198", "queen david", "zzz", "",
        ] {
            let folded = row_match::fold_needle(needle);
            assert_eq!(
                key.matches(&folded),
                row_match::track_matches(&r, &folded),
                "packed key and track_matches disagree on {needle:?} for row {}",
                r.id
            );
        }
    }
}
