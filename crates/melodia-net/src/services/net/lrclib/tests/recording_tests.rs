//! Cases taken from rows the directory actually returned, plus the ones it would be wrong to
//! accept. Every case names the tags on both sides, since what is being tested is a judgement
//! about two spellings rather than a function of one.

use super::*;

/// `(our title, our artist)` against `(their title, their artist)`.
fn same(ours: (&str, &str), theirs: (&str, &str)) -> bool {
    Recording::new(ours.0, ours.1).matches(&Recording::new(theirs.0, theirs.1))
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
fn the_query_drops_the_feat_credit_and_the_guests_after_the_first() {
    assert_eq!(query_title("Eclipse (feat. Dr Disrespect)"), "Eclipse");
    assert_eq!(query_title("Let Me Go ft. Hailee Steinfeld"), "Let Me Go");
    assert_eq!(query_title("Believer (Live)"), "Believer (Live)", "a version marker stays");
    assert_eq!(query_artist("Dominic Strike, Euphoria"), "Dominic Strike");
    assert_eq!(query_artist("J+1\u{0}DrDisrespect"), "J+1");
    assert_eq!(query_artist("Alesso & Hailee Steinfeld"), "Alesso");
}
