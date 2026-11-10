//! What makes two tracks one song, how sure the match is, and which copy the picks keep.

use melodia_core::entities::audio_quality::AudioFormat;

use super::*;

/// A length the copies share unless a test moves one.
const LENGTH_MS: i64 = 200_000;

/// An MP3 of "Song" by "Artist", track 1 of "Album" (2000), with no hash and no MBIDs: a copy each
/// test edits only where its case differs.
fn copy(id: i64) -> DuplicateCopy {
    DuplicateCopy {
        id,
        file_path: format!("{id}.mp3"),
        file_hash: None,
        file_size: Some(5_000_000),
        span: None,
        title: "Song".to_owned(),
        artist: Some("Artist".to_owned()),
        album: Some("Album".to_owned()),
        disc_number: Some(1),
        track_number: Some(1),
        year: Some(2000),
        musicbrainz_track_id: None,
        musicbrainz_release_id: None,
        duration_ms: LENGTH_MS,
        date_added: "2024-01-01T00:00:00+00:00".to_owned(),
        format: AudioFormat {
            codec: Some("Mpeg".to_owned()),
            bitrate: Some(320),
            sample_rate: Some(44_100),
            bit_depth: None,
        },
        dismissal: None,
        play_count: 0,
        rating: 0,
        is_favorite: false,
        playlist_count: 0,
    }
}

/// Each group's copy ids, keeper first.
fn ids(groups: &[DuplicateGroup]) -> Vec<Vec<i64>> {
    groups.iter().map(|group| group.copies.iter().map(|copy| copy.id).collect()).collect()
}

/// Each group's copy ids in id order, for the tests about who groups rather than who is kept.
fn members(groups: &[DuplicateGroup]) -> Vec<Vec<i64>> {
    let mut members = ids(groups);
    for group in &mut members {
        group.sort_unstable();
    }
    members
}

fn kinds(groups: &[DuplicateGroup]) -> Vec<MatchKind> {
    groups.iter().map(|group| group.kind).collect()
}

fn titled(id: i64, artist: Option<&str>, title: &str) -> DuplicateCopy {
    DuplicateCopy { artist: artist.map(str::to_owned), title: title.to_owned(), ..copy(id) }
}

#[test]
fn copies_within_the_tolerance_of_the_shortest_group_together() {
    let rows = [
        (DURATION_TOLERANCE_MS - 1, vec![vec![1, 2]]),
        (DURATION_TOLERANCE_MS, vec![vec![1, 2]]),
        (DURATION_TOLERANCE_MS + 1, vec![]),
    ];
    for (apart, expected) in rows {
        let longer = DuplicateCopy { duration_ms: LENGTH_MS + apart, ..copy(2) };

        let groups = group(vec![copy(1), longer]);

        assert_eq!(members(&groups), expected, "{apart} ms apart");
    }
}

#[test]
fn a_run_of_near_misses_does_not_chain_into_one_group() {
    let step = DURATION_TOLERANCE_MS * 2 / 3;
    let second = DuplicateCopy { duration_ms: LENGTH_MS + step, ..copy(2) };
    let third = DuplicateCopy { duration_ms: LENGTH_MS + 2 * step, ..copy(3) };

    let groups = group(vec![copy(1), second, third]);

    assert_eq!(members(&groups), [[1, 2]]);
}

#[test]
fn spellings_that_differ_only_in_case_accents_or_punctuation_group() {
    let tagged = titled(1, Some("Beyonc\u{e9}"), "Don't Stop (Live)");
    let retyped = titled(2, Some("BEYONCE"), "dont stop - live");

    let groups = group(vec![tagged, retyped]);

    assert_eq!(members(&groups), [[1, 2]]);
}

/// Pass 2 is handed whatever shared a key hash, so it must tell the words apart itself.
#[test]
fn different_songs_never_group_whatever_their_length() {
    let rows = [
        ("another title", titled(2, Some("Artist"), "Other Song")),
        ("another artist", titled(2, Some("Other Artist"), "Song")),
    ];
    for (label, other) in rows {
        let groups = group(vec![copy(1), other]);

        assert!(groups.is_empty(), "{label}");
    }
}

/// An edit to the second copy, or to both.
type Differ = fn(&mut DuplicateCopy, &mut DuplicateCopy);

#[test]
fn each_group_takes_the_kind_its_strongest_doubt_names() {
    use ReviewReason::{NoArtist, OtherPosition, OtherRecording, OtherRelease};
    let review = MatchKind::Review;
    let rows: [(&str, Differ, MatchKind); 19] = [
        ("the same tags", |_, _| {}, MatchKind::SameRelease),
        (
            "one file",
            |a, b| {
                a.file_hash = Some("h".to_owned());
                b.file_hash = Some("h".to_owned());
            },
            MatchKind::Identical,
        ),
        (
            "one file with no artist",
            |a, b| {
                (a.file_hash, b.file_hash) = (Some("h".to_owned()), Some("h".to_owned()));
                (a.artist, b.artist) = (None, None);
            },
            MatchKind::Identical,
        ),
        ("one copy hashed", |a, _| a.file_hash = Some("h".to_owned()), MatchKind::SameRelease),
        ("no artist", |a, b| (a.artist, b.artist) = (None, None), review(NoArtist)),
        (
            "no artist and two recordings",
            |a, b| {
                (a.artist, b.artist) = (None, None);
                a.musicbrainz_track_id = Some("r1".to_owned());
                b.musicbrainz_track_id = Some("r2".to_owned());
            },
            review(NoArtist),
        ),
        (
            "two recordings",
            |a, b| {
                a.musicbrainz_track_id = Some("r1".to_owned());
                b.musicbrainz_track_id = Some("r2".to_owned());
            },
            review(OtherRecording),
        ),
        (
            "one recording tagged",
            |a, _| a.musicbrainz_track_id = Some("r1".to_owned()),
            MatchKind::SameRelease,
        ),
        (
            "two recordings on two albums",
            |a, b| {
                a.musicbrainz_track_id = Some("r1".to_owned());
                b.musicbrainz_track_id = Some("r2".to_owned());
                b.album = Some("Compilation".to_owned());
            },
            review(OtherRecording),
        ),
        ("two albums", |_, b| b.album = Some("Compilation".to_owned()), review(OtherRelease)),
        ("an album beside a loose single", |_, b| b.album = None, review(OtherRelease)),
        (
            "one album spelled twice",
            |_, b| b.album = Some("ALBUM!".to_owned()),
            MatchKind::SameRelease,
        ),
        ("a remaster's year", |_, b| b.year = Some(2011), review(OtherRelease)),
        ("one year tagged", |_, b| b.year = None, MatchKind::SameRelease),
        (
            "two releases",
            |a, b| {
                a.musicbrainz_release_id = Some("e1".to_owned());
                b.musicbrainz_release_id = Some("e2".to_owned());
            },
            review(OtherRelease),
        ),
        (
            "two albums at two positions",
            |_, b| {
                b.album = Some("Compilation".to_owned());
                b.track_number = Some(7);
            },
            review(OtherRelease),
        ),
        ("two track numbers", |_, b| b.track_number = Some(7), review(OtherPosition)),
        ("two discs", |_, b| b.disc_number = Some(2), review(OtherPosition)),
        ("one position tagged", |_, b| b.track_number = None, MatchKind::SameRelease),
    ];
    for (label, differ, expected) in rows {
        let (mut first, mut second) = (copy(1), copy(2));
        differ(&mut first, &mut second);

        let groups = group(vec![first, second]);

        assert_eq!(kinds(&groups), [expected], "{label}");
    }
}

#[test]
fn the_keeper_is_the_better_encode_whatever_its_bitrate() {
    let flac = DuplicateCopy {
        format: AudioFormat {
            codec: Some("Flac".to_owned()),
            bitrate: Some(216),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
        },
        date_added: "2025-01-01T00:00:00+00:00".to_owned(),
        ..copy(2)
    };

    let groups = group(vec![copy(1), flac]);

    assert_eq!(ids(&groups), [[2, 1]]);
}

#[test]
fn of_two_equal_encodes_the_keeper_is_the_one_added_first() {
    let older = DuplicateCopy { date_added: "2020-01-01T00:00:00+00:00".to_owned(), ..copy(2) };

    let groups = group(vec![copy(1), older]);

    assert_eq!(ids(&groups), [[2, 1]]);
}

#[test]
fn of_two_equal_encodes_added_together_the_keeper_is_the_lower_id() {
    let groups = group(vec![copy(5), copy(3)]);

    assert_eq!(ids(&groups), [[3, 5]]);
}

#[test]
fn groups_list_by_kind_then_artist_then_title() {
    let hashed = |id| DuplicateCopy {
        file_hash: Some("h".to_owned()),
        ..titled(id, Some("Zed"), "Zed Song")
    };
    let other_album = |id| DuplicateCopy {
        album: Some(format!("Album {id}")),
        ..titled(id, Some("Artist"), "Alpha")
    };
    let copies = vec![
        titled(1, Some("Beta"), "Song"),
        titled(2, Some("Beta"), "Song"),
        other_album(3),
        other_album(4),
        titled(5, None, "Song"),
        titled(6, None, "Song"),
        titled(7, Some("Alpha"), "Song"),
        titled(8, Some("Alpha"), "Song"),
        titled(9, Some("Alpha"), "Ballad"),
        titled(10, Some("Alpha"), "Ballad"),
        hashed(11),
        hashed(12),
    ];

    let groups = group(copies);

    let listed: Vec<(MatchKind, Option<&str>, &str)> = groups
        .iter()
        .filter_map(|group| {
            let lead = group.copies.first()?;
            Some((group.kind, lead.artist.as_deref(), lead.title.as_str()))
        })
        .collect();
    assert_eq!(
        listed,
        [
            (MatchKind::Identical, Some("Zed"), "Zed Song"),
            (MatchKind::SameRelease, Some("Alpha"), "Ballad"),
            (MatchKind::SameRelease, Some("Alpha"), "Song"),
            (MatchKind::SameRelease, Some("Beta"), "Song"),
            (MatchKind::Review(ReviewReason::NoArtist), None, "Song"),
            (MatchKind::Review(ReviewReason::OtherRelease), Some("Artist"), "Alpha"),
        ]
    );
}

/// Keyed on the folded words the groups compare, or a respelled copy never reaches pass 2.
#[test]
fn only_keys_that_collide_become_candidates() {
    let row = |id, artist: &str, title: &str| DuplicateKeyRow {
        id,
        title: title.to_owned(),
        artist: Some(artist.to_owned()),
    };
    let keys = vec![
        SongKey::of(&row(1, "Artist", "Song")),
        SongKey::of(&row(2, "Other", "Lone")),
        SongKey::of(&row(3, "ARTIST", "song")),
    ];

    let mut candidates = candidate_ids(keys);
    candidates.sort_unstable();

    assert_eq!(candidates, [1, 3]);
}
