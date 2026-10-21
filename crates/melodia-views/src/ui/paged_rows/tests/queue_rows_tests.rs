//! The row shape the queue sheet and Up Next both draw.

use super::to_queue_row;
use crate::ui::fixtures::track_row;
use crate::ui::util::StringPool;
use melodia_core::entities::track::TrackListRow;

fn queue_row(row: &TrackListRow) -> melodia_ui::QueueRow {
    to_queue_row(row, &mut StringPool::default())
}

/// The track-list projection carries the foreign keys, which is what retired the link fetch the
/// sheet used to stamp them with after the rows landed.
#[test]
fn a_row_carries_the_ids_its_menu_goes_to() {
    let row = TrackListRow {
        album_id: Some(1),
        artist_id: Some(2),
        genre_id: None,
        ..track_row(7, "Alpha")
    };

    let queued = queue_row(&row);

    assert_eq!((queued.album_id, queued.artist_id, queued.genre_id), (1, 2, 0));
    assert_eq!(queued.id, 7);
    assert!(!queued.selected);
}

#[test]
fn a_row_with_nothing_to_say_renders_empty_rather_than_absent() {
    let queued = queue_row(&track_row(1, "Alpha"));

    assert_eq!(queued.artist.as_str(), "");
    assert_eq!(queued.artwork_path.as_str(), "");
    assert_eq!(queued.title.as_str(), "Alpha");
}

/// A negative duration is nonsense a row still has to draw, and the unclamped arithmetic renders
/// it as `0:-1` — the minute and the second are floored independently.
#[test]
fn a_negative_duration_is_drawn_as_no_time_at_all() {
    let row = TrackListRow { duration_ms: -1_000, ..track_row(1, "Alpha") };

    assert_eq!(queue_row(&row).display_duration.as_str(), "0:00");
}
