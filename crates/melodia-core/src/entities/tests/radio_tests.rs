//! The two answers a station's format is folded out of, and the one case where folding twice was
//! the bug.
//!
//! [`RadioStation::format`] itself is left out: it is one line delegating to [`compose_format`],
//! and the argument order it could get wrong does not typecheck, the directory's word being a
//! `&str` where the measurement is an `Option`.

use std::borrow::Cow;

use super::super::radio::{compose_format, contains_ignore_ascii_case, recompose_format};

/// Every arm, and the two blanks either side of them.
///
/// `UNKNOWN` appears twice on purpose: it names nothing only once there is a measurement to put in
/// its place, and alone it is still the only word the directory offered.
#[test]
fn the_directorys_word_and_a_measurement_fold_into_one_format() {
    let cases: [(&str, Option<&str>, Option<&str>); 11] = [
        ("", None, None),
        ("", Some(""), None),
        ("OGG", None, Some("OGG")),
        ("", Some("OPUS"), Some("OPUS")),
        ("UNKNOWN", None, Some("UNKNOWN")),
        ("UNKNOWN", Some("OPUS"), Some("OPUS")),
        ("unknown", Some("OPUS"), Some("OPUS")),
        ("AAC+", Some("AAC"), Some("AAC+")),
        ("AAC", Some("AAC+"), Some("AAC+")),
        ("MP3", Some("mp3"), Some("MP3")),
        ("OGG", Some("VORBIS"), Some("OGG/VORBIS")),
    ];

    for (directory, probed, expected) in cases {
        assert_eq!(
            compose_format(directory, probed).as_deref(),
            expected,
            "directory {directory:?}, probed {probed:?}"
        );
    }
}

/// A station row draws far more stations carrying one answer than two, so the arms that hand back
/// what they were given must not build a string on the way.
#[test]
fn only_a_join_builds_a_string() {
    let borrowed =
        [("OGG", None), ("", Some("OPUS")), ("UNKNOWN", Some("OPUS")), ("AAC+", Some("AAC"))];

    for (directory, probed) in borrowed {
        assert!(
            matches!(compose_format(directory, probed), Some(Cow::Borrowed(_))),
            "directory {directory:?}, probed {probed:?} allocated a copy of an answer it was handed"
        );
    }
    assert!(matches!(compose_format("OGG", Some("VORBIS")), Some(Cow::Owned(_))));
}

/// A Now-Playing surface holds a value that already carries a measurement, so composing against it
/// a second time is composing against a composition.
///
/// The containment arms absorb a repeat, which is what made this look safe: it only breaks for a
/// station that changed what it serves between two plays, and then it names both codecs for a
/// mount that has one.
#[test]
fn a_station_that_changed_what_it_serves_replaces_the_measurement() {
    assert_eq!(recompose_format("OGG/VORBIS", "OPUS").as_deref(), Some("OGG/OPUS"));
}

#[test]
fn a_station_still_serving_what_it_served_is_not_named_twice() {
    assert_eq!(recompose_format("OGG/VORBIS", "VORBIS").as_deref(), Some("OGG/VORBIS"));
}

/// Every station the user has not played yet carries the directory's word alone, so the strip is a
/// no-op far more often than it is a strip.
#[test]
fn a_value_that_was_never_composed_folds_like_a_fresh_one() {
    assert_eq!(recompose_format("OGG", "OPUS").as_deref(), Some("OGG/OPUS"));
    assert_eq!(recompose_format("", "OPUS").as_deref(), Some("OPUS"));
}

/// `windows(0)` panics rather than answering false, and an empty needle reaches here from a
/// station whose `probed_codec` column holds `""` rather than `NULL`.
#[test]
fn an_empty_needle_is_refused_rather_than_windowed() {
    assert!(!contains_ignore_ascii_case("OGG", ""));
    assert!(!contains_ignore_ascii_case("", ""));
}

#[test]
fn a_needle_longer_than_its_haystack_is_refused() {
    assert!(!contains_ignore_ascii_case("AAC", "AAC+"));
    assert!(contains_ignore_ascii_case("AAC+", "AAC"));
}

/// The fold is ASCII-only, which is what lets it compare in place. A directory name carrying
/// anything else is matched byte for byte, and a window opening mid-character cannot match an
/// ASCII needle.
#[test]
fn the_fold_reaches_ascii_and_stops_there() {
    assert!(contains_ignore_ascii_case("Rádio OPUS", "opus"));
    assert!(!contains_ignore_ascii_case("Rádio", "RADIO"));
}
