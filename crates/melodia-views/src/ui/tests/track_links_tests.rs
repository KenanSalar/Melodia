//! The FK trio the now-playing bar's menu reads, and the gate it reads it through.

use super::{RowLinks, to_row_id};
use melodia_core::entities::track::TrackLinks;

fn links(album: Option<i64>, artist: Option<i64>, genre: Option<i64>) -> TrackLinks {
    TrackLinks { id: 1, album_id: album, artist_id: artist, genre_id: genre }
}

/// **Zero is "we don't know".** The menu gates each entry on `album-id != 0`, so a linkage the
/// track genuinely lacks and one the fetch has not landed yet read the same — which is the honest
/// answer until it lands.
#[test]
fn a_linkage_the_track_lacks_reads_as_the_same_zero_a_pending_fetch_does() {
    assert_eq!(to_row_id(None), 0);
    assert_eq!(to_row_id(Some(7)), 7);
}

/// An id past what a row can hold is unresolved rather than truncated — a wrapped one would send
/// the menu to whatever entity happened to land on that number.
#[test]
fn an_id_a_row_cannot_hold_reads_as_unresolved() {
    assert_eq!(to_row_id(Some(i64::from(i32::MAX) + 1)), 0);
    assert_eq!(to_row_id(Some(i64::MAX)), 0);
    assert_eq!(to_row_id(Some(i64::from(i32::MAX))), i32::MAX);
}

#[test]
fn every_absent_id_comes_across_as_zero() {
    let trio = RowLinks::from(links(None, Some(2), None));

    assert_eq!((trio.album, trio.artist, trio.genre), (0, 2, 0));
}
