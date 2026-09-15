//! The FK trio a queue row carries, and the gate the menu reads it through.

use super::{RowLinks, to_row_id};
use melodia_core::entities::track::TrackLinks;
use melodia_ui::QueueRow;

fn links(album: Option<i64>, artist: Option<i64>, genre: Option<i64>) -> TrackLinks {
    TrackLinks { id: 1, album_id: album, artist_id: artist, genre_id: genre }
}

fn row(album: i32, artist: i32, genre: i32) -> QueueRow {
    QueueRow { album_id: album, artist_id: artist, genre_id: genre, ..QueueRow::default() }
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

/// **The `bool` is what the patch walk is for.** `set_row_data` re-runs the delegate's bindings, so
/// stamping a row that already holds these ids would repaint it for no change.
#[test]
fn stamping_a_row_that_already_holds_the_trio_reports_nothing_moved() {
    let trio = RowLinks::from(links(Some(1), Some(2), Some(3)));
    let mut already = row(1, 2, 3);

    assert!(!trio.stamp(&mut already));
    assert_eq!((already.album_id, already.artist_id, already.genre_id), (1, 2, 3));
}

/// Any one of the three moving is enough, so the walk cannot skip a row whose genre arrived while
/// its album was already known.
#[test]
fn one_field_moving_is_enough_to_report_the_row_changed() {
    let trio = RowLinks::from(links(Some(1), Some(2), Some(3)));

    for mut stale in [row(0, 2, 3), row(1, 0, 3), row(1, 2, 0)] {
        assert!(trio.stamp(&mut stale));
        assert_eq!((stale.album_id, stale.artist_id, stale.genre_id), (1, 2, 3));
    }
}
