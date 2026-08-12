//! The pure folds, tested without an `AppWindow`.
//!
//! They moved out of `hero_chips_tests.rs` with the folds themselves: nothing
//! here builds a chip or touches the published record, so the two files split
//! along the same seam their sources do.

use super::{
    HeroFold, MostPlayedTotals, dominant_genre, fold_most_played, fold_tracks, year_span,
};
use crate::entities::album::AlbumStats;
use crate::entities::track::{MostPlayedFavorite, TrackListRow};

/// Only the four fields the folds and the genre tally read.
fn track(artist_id: Option<i64>, album_id: Option<i64>, genre: Option<&str>) -> TrackListRow {
    TrackListRow {
        id: 0,
        file_path: String::new(),
        file_name: String::new(),
        title: String::new(),
        artist: None,
        album_artist: None,
        album: None,
        genre: genre.map(str::to_owned),
        track_number: None,
        disc_number: None,
        year: None,
        duration_ms: 0,
        artwork_path: None,
        is_favorite: false,
        rating: 0,
        album_id,
        artist_id,
        genre_id: None,
        date_added: String::new(),
        sort_key: None,
    }
}

fn played(duration_ms: i64, play_count: i32) -> MostPlayedFavorite {
    MostPlayedFavorite {
        id: 0,
        title: String::new(),
        artist: None,
        album_artist: None,
        album: None,
        genre: None,
        year: None,
        artwork_path: None,
        play_count,
        duration_ms,
    }
}

/// Only the one field [`year_span`] reads.
fn dated_album(year: Option<i32>) -> AlbumStats {
    AlbumStats {
        id: 1,
        name: "Kind of Blue".into(),
        sort_name: None,
        artist_id: 1,
        artist_name: "Miles Davis".into(),
        year,
        disc_count: None,
        is_compilation: false,
        musicbrainz_id: None,
        artwork_path: None,
        track_count: 5,
        total_duration_ms: 2_733_000,
    }
}

#[test]
fn the_fold_counts_distinct_ids_and_skips_the_untagged() {
    // A track with no album belongs to none, so it is skipped rather than
    // pooled into an "unknown" bucket that would read as one more album.
    let rows = [
        track(Some(1), Some(10), None),
        track(Some(1), Some(10), None),
        track(Some(2), Some(11), None),
        track(None, None, None),
    ];
    assert_eq!(fold_tracks(&rows), HeroFold { artists: 2, albums: 2 });
    assert_eq!(fold_tracks(&[]), HeroFold::default());
}

#[test]
fn most_played_totals_sum_duration_and_plays() {
    let rows = [played(180_000, 12), played(240_000, 30)];
    assert_eq!(
        fold_most_played(&rows),
        MostPlayedTotals {
            tracks: 2,
            duration_ms: 420_000,
            plays: 42,
        }
    );
}

#[test]
fn a_genre_is_named_only_when_it_actually_dominates() {
    let mostly_jazz = [
        track(None, None, Some("Jazz")),
        track(None, None, Some("Jazz")),
        track(None, None, Some("Blues")),
    ];
    assert_eq!(dominant_genre(&mostly_jazz).as_deref(), Some("Jazz"));

    // An even split has no majority — naming either would misrepresent the
    // other half, so a genuinely mixed compilation gets no chip.
    let split = [
        track(None, None, Some("Jazz")),
        track(None, None, Some("Blues")),
    ];
    assert_eq!(dominant_genre(&split), None);

    // Untagged tracks don't count toward the total, so one tagged track among
    // three still dominates the tracks that have a genre at all.
    let sparse = [
        track(None, None, Some("Jazz")),
        track(None, None, None),
        track(None, None, Some("")),
    ];
    assert_eq!(dominant_genre(&sparse).as_deref(), Some("Jazz"));
    assert_eq!(dominant_genre(&[]), None);
}

#[test]
fn the_year_span_ignores_albums_with_no_year() {
    assert_eq!(
        year_span(&[dated_album(Some(1963)), dated_album(None), dated_album(Some(1957))]),
        Some((1957, 1963))
    );
    assert_eq!(year_span(&[dated_album(Some(0)), dated_album(None)]), None);
    assert_eq!(year_span(&[]), None);
}
