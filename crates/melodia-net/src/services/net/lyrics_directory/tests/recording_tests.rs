//! Cases taken from rows the directory actually returned, plus the ones it would be wrong to
//! accept. Every case names the tags on both sides, since what is being tested is a judgement
//! about two spellings rather than a function of one.
//!
//! Our side has two of those spellings where a file lists its artists, so a case built through
//! [`tagged`] is testing something [`same`] cannot reach: the row is one string either way.

use super::*;

/// `(our title, our artist)` against `(their title, their artist)`.
fn same(ours: (&str, &str), theirs: (&str, &str)) -> bool {
    Recording::new(ours.0, ours.1).matches(&Recording::new(theirs.0, theirs.1))
}

/// [`same`] with the credit behind our tags rather than its printed line alone.
fn same_credit(ours: (&str, &ArtistCredit), theirs: (&str, &str)) -> bool {
    Recording::from_credit(ours.0, ours.1).matches(&Recording::new(theirs.0, theirs.1))
}

/// The credit a file's two artist tags give: `ARTIST` as printed, `ARTISTS` one value per name.
/// An empty `names` is the single-value tag most libraries carry.
fn tagged(printed: &str, names: &[&str]) -> ArtistCredit {
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    ArtistCredit::from_tags(printed, &names)
}

#[test]
fn a_row_crediting_a_guest_the_file_leaves_out_is_the_same_recording() {
    assert!(
        same(("Heartbeat", "Dominic Strike"), ("Heartbeat", "Dominic Strike, Euphoria")),
        "the timed row for this track is filed under both artists where the tag names one"
    );
}

#[test]
fn a_feat_credit_in_their_title_is_not_a_different_song() {
    assert!(
        same(("Eclipse", "DrDisrespect"), ("Eclipse (feat. Dr Disrespect)", "DrDisrespect")),
        "the guest moved into the title is the same recording under a different filing"
    );
}

#[test]
fn a_nul_joined_credit_still_finds_the_artist_in_it() {
    assert!(
        same(("Eclipse", "DrDisrespect"), ("Eclipse", "J+1\u{0}DrDisrespect")),
        "ID3v2.4 joins multiple values with a NUL, and so do the rows written from such tags"
    );
}

#[test]
fn a_collaboration_credited_with_an_x_is_split_the_same_way_the_query_splits_it() {
    assert!(
        same(("Raging", "Kygo x Romy Wave"), ("Raging", "Kygo, Romy Wave")),
        "the query is cut at the first credit, so the check has to read the rest as credits too"
    );
    assert_eq!(query_artist("Kygo x Romy Wave"), "Kygo");
}

#[test]
fn another_artists_song_of_the_same_name_is_refused() {
    assert!(
        !same(("Eclipse", "DrDisrespect"), ("Eclipse", "Pink Floyd")),
        "the index is a keyword search, so a same-titled song by somebody else is what it \
         returns rather than an edge case"
    );
}

#[test]
fn a_live_take_is_not_the_studio_cut() {
    assert!(
        !same(("Believer", "Imagine Dragons"), ("Believer (Live)", "Imagine Dragons")),
        "a re-performance sings the same words in different places, so its sheet cannot follow \
         this recording"
    );
    assert!(
        !same(("Believer (Live)", "Imagine Dragons"), ("Believer", "Imagine Dragons")),
        "and the mismatch is refused from either side"
    );
}

#[test]
fn a_remaster_is_the_same_performance() {
    assert!(
        same(("Song", "Artist"), ("Song (Remastered 2011)", "Artist")),
        "an editorial note on one recording, where a live take is a second recording"
    );
}

#[test]
fn an_original_mix_is_not_read_as_a_remix() {
    assert!(
        same(("Track", "Artist"), ("Track (Original Mix)", "Artist")),
        "half an electronic library labels the plain track this way, so `mix` is deliberately \
         not a version marker"
    );
    assert!(!same(("Track", "Artist"), ("Track (Tiesto Remix)", "Artist")), "where `remix` is one");
}

#[test]
fn punctuation_and_accents_do_not_separate_two_spellings_of_one_title() {
    assert!(same(("Deja Vu", "Artist"), ("Déjà vu", "Artist")));
    assert!(same(("Dont Stop  Me", "Artist"), ("Don't stop me", "Artist")));
}

#[test]
fn a_title_that_reduces_to_nothing_matches_nothing() {
    assert!(
        !same(("(Live)", "Artist"), ("(Live)", "Artist")),
        "a row whose whole title is a bracketed aside carries no words to compare, and an empty \
         core would otherwise match every other empty one"
    );
}

#[test]
fn undercover_is_not_a_cover() {
    assert!(
        same(("Undercover", "Artist"), ("Undercover", "Artist")),
        "the markers are matched as words, not as substrings"
    );
}

#[test]
fn a_marker_word_in_the_title_proper_is_not_a_version_marker() {
    // One occurrence against two, the core title being the same either way.
    assert!(
        !same(("Live Echo", "Artist"), ("Live Echo (Live)", "Artist")),
        "a live recording is not the studio cut, whatever the title happens to be called"
    );
    assert!(
        same(("Live Echo", "Artist"), ("Live Echo", "Artist")),
        "and the song still matches itself"
    );
}

#[test]
fn a_marked_take_matches_the_same_take() {
    // Counting must not make two spellings of one live version unequal, which is the direction
    // that would cost sheets rather than mismatch them.
    assert!(same(("Believer (Live)", "Artist"), ("Believer (Live)", "Artist")));
}

#[test]
fn a_slash_separates_two_credited_artists() {
    // It is split before the fold rather than after, alongside the NUL, so it never reaches the
    // comparison as a character inside one long credit.
    assert!(same(("Song", "Alpha"), ("Song", "Alpha/Beta")));
}

#[test]
fn a_typographic_apostrophe_is_the_same_word_as_a_typed_one() {
    // The one a phone keyboard and Apple Music both write, where a tagger types the ASCII form.
    // Spaced rather than dropped it would split the word and never match again.
    assert!(same(("Don't Stop", "Artist"), ("Don\u{2019}t Stop", "Artist")));
}

#[test]
fn a_credit_that_is_nothing_but_separators_can_match_nothing() {
    // Asked before a request is spent rather than after the answer is read, so a tag like this
    // costs no traffic at all.
    assert!(!Recording::new("Song", " , ; ").can_match());
    assert!(Recording::new("Song", "Artist").can_match());
}

#[test]
fn the_query_drops_the_feat_credit_and_the_guests_after_the_first() {
    assert_eq!(query_title("Eclipse (feat. Dr Disrespect)"), "Eclipse");
    assert_eq!(query_title("Let Me Go ft. Hailee Steinfeld"), "Let Me Go");
    assert_eq!(query_title("Believer (Live)"), "Believer (Live)", "a version marker stays");
    assert_eq!(query_artist("Dominic Strike, Euphoria"), "Dominic Strike");
    assert_eq!(query_artist("J+1\u{0}DrDisrespect"), "J+1");
    assert_eq!(query_artist("Alesso & Hailee Steinfeld"), "Alesso");
}

#[test]
fn a_join_phrase_no_splitter_carries_still_finds_the_row_filed_under_the_lead() {
    let ours = tagged("Alesso vs. Hailee Steinfeld", &["Alesso", "Hailee Steinfeld"]);

    assert!(
        same_credit(("Let Me Go", &ours), ("Let Me Go", "Alesso")),
        "the names are what the file lists, so who leads is stated rather than guessed at"
    );
    assert!(
        !same(("Let Me Go", "Alesso vs. Hailee Steinfeld"), ("Let Me Go", "Alesso")),
        "and the printed line alone cannot find it: nothing splits `vs.`, so the whole credit \
         reads as one name nobody is filed under"
    );
}

#[test]
fn a_name_that_contains_a_splitter_still_matches_a_row_crediting_a_guest() {
    // The reading that finds the case above is a second set rather than more of the first, and
    // this is the case that costs: `credits` always cuts ` & `, so a row can never carry
    // `simon garfunkel` as one name and a merged set is a subset of nothing.
    let ours = tagged("Simon & Garfunkel", &[]);

    assert!(
        same_credit(("The Boxer", &ours), ("The Boxer", "Simon & Garfunkel, Aretha Franklin")),
        "one band whose own name reads as two credits, against a row that adds a guest"
    );
    assert!(
        same_credit(("The Boxer", &ours), ("The Boxer", "Simon & Garfunkel")),
        "and against the row that spells it exactly as the tag does"
    );
}

#[test]
fn a_credited_name_does_not_make_another_artists_song_of_the_same_name_ours() {
    let ours = tagged("Alesso vs. Hailee Steinfeld", &["Alesso", "Hailee Steinfeld"]);

    assert!(
        !same_credit(("Let Me Go", &ours), ("Let Me Go", "Pink Floyd")),
        "reading the names beside the split widens what matches, and the widening may not reach \
         a row sharing no name at all"
    );
}

#[test]
fn the_query_takes_the_lead_off_a_credit_and_guesses_only_without_one() {
    assert_eq!(
        search_artist(&tagged("Alesso vs. Hailee Steinfeld", &["Alesso", "Hailee Steinfeld"])),
        "Alesso"
    );
    assert_eq!(
        search_artist(&tagged("Alesso & Hailee Steinfeld", &[])),
        "Alesso",
        "a single-value tag may still be several artists run together, so the guess stands"
    );
    assert_eq!(
        search_artist(&tagged("Alesso vs. Hailee Steinfeld", &[])),
        "Alesso vs. Hailee Steinfeld",
        "and where the guess finds no separator it leaves the string whole"
    );
}
