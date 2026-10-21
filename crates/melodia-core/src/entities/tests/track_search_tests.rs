//! What `tracks.search_text` holds. Every filled row keeps the text these pin until a migration
//! blanks the column, so a change here that ships without one leaves the library matching two
//! spellings at once.

use super::SearchFields;
use crate::utils::needle::fold_needle;

const JOGA: SearchFields<'static> = SearchFields {
    title: "Jóga",
    artist: "Björk",
    album_artist: "",
    album: "Homogenic",
    genre: "Art Pop",
};

#[test]
fn the_packed_text_is_each_field_folded_and_joined_by_a_newline() {
    let fields = SearchFields {
        title: "Jóga",
        artist: "Björk",
        album_artist: "BJÖRK",
        album: "Homogenic",
        genre: "Art Pop",
    };

    let packed = fields.pack();

    assert_eq!(packed, "joga\nbjork\nbjork\nhomogenic\nart pop");
}

/// An empty column is the backfill's "not filled yet", so a row with nothing to search still has
/// to pack to something.
#[test]
fn five_empty_fields_still_pack_to_four_separators() {
    let fields = SearchFields { title: "", artist: "", album_artist: "", album: "", genre: "" };

    let packed = fields.pack();

    assert_eq!(packed, "\n\n\n\n");
}

/// ID3v2.4 joins a multi-value frame on NUL, and `SQLite` leaves a NUL inside TEXT undefined.
#[test]
fn a_nul_inside_a_field_packs_as_a_space() {
    let fields = SearchFields {
        title: "",
        artist: "Simon\0Garfunkel",
        album_artist: "",
        album: "",
        genre: "",
    };

    let packed = fields.pack();

    assert_eq!(packed, "\nsimon garfunkel\n\n\n");
}

/// The store matches with one `instr` over the whole packed text, where the lists held in memory
/// ask each field on its own. Joined with nothing between them, "Sofa" and "Bach" would read as
/// "sofabach" and match a needle neither field holds.
#[test]
fn no_needle_matches_across_two_fields() {
    let fields =
        SearchFields { title: "Sofa", artist: "Bach", album_artist: "", album: "", genre: "" };

    let packed = fields.pack();

    assert!(!packed.contains(fold_needle("ab").as_str()), "{packed:?}");
}

/// The store's answer and the in-memory one, over one partition each: whether the packed text
/// holds the needle has to be whether some field does.
#[test]
fn the_packed_text_holds_a_needle_exactly_when_a_field_does() {
    const CASES: [(&str, bool, &str); 7] = [
        ("homo", true, "an ascii needle on an ascii field"),
        ("JÓG", true, "an accented needle on an accented field"),
        ("bjork", true, "a plain needle on an accented field"),
        ("hómo", true, "an accented needle on a plain field"),
        ("ART", true, "a needle differing only in case"),
        ("zzz", false, "a needle no field holds"),
        ("", true, "the empty needle, which matches every row"),
    ];
    let packed = JOGA.pack();
    for (raw, expected, case) in CASES {
        let needle = fold_needle(raw);

        let in_packed = packed.contains(needle.as_str());
        let in_a_field = JOGA.as_array().iter().any(|field| needle.contains(field));

        assert_eq!((in_packed, in_a_field), (expected, expected), "{case}");
    }
}
