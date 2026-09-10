//! The rebuild's fast path, and the row shape both surfaces draw.
//!
//! `same_sources` decides whether a published queue is rebuilt or merely re-selected, and it
//! answers from pointers rather than fields on purpose. A test comparing content would agree with
//! it on every case that matters and disagree on the one it exists for.

use std::sync::Arc;

use super::{same_sources, to_slint_queue_row};
use crate::ui::queue_sheet::ShadowEntry;
use melodia_core::entities::track::TrackSummary;

fn summary(id: i64, title: &str) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: String::new(),
        file_name: String::new(),
        title: title.to_owned(),
        artist: None,
        album: None,
        duration_ms: 180_000,
        artwork_path: None,
        track_number: None,
        disc_number: None,
        last_position: 0,
        is_favorite: false,
        rating: 0,
        replaygain_track_gain: None,
        replaygain_track_peak: None,
        replaygain_album_gain: None,
        replaygain_album_peak: None,
    })
}

/// The shadow a rebuild leaves behind, holding the summaries it was built from.
fn shadow(tracks: &[Arc<TrackSummary>]) -> Vec<ShadowEntry> {
    tracks
        .iter()
        .map(|track| ShadowEntry {
            id: track.id,
            selected: false,
            source: Some(Arc::clone(track)),
        })
        .collect()
}

#[test]
fn a_queue_republished_unchanged_is_the_same_sources() {
    let tracks = [summary(1, "Alpha"), summary(2, "Beta")];

    assert!(same_sources(&shadow(&tracks), &tracks));
}

/// **`Arc::make_mut` mutates in place wherever it is uniquely owned**, so a tag edit on a queued
/// track can land at the address the shadow already holds. The shadow's own strong reference is
/// what forces the clone; comparing fields here instead would call the fresh summary equal to the
/// stale one and leave the sheet drawing the old title.
#[test]
fn a_summary_replaced_by_an_equal_one_is_not_the_same_sources() {
    let before = [summary(1, "Alpha")];
    let seen = shadow(&before);
    let after = [summary(1, "Alpha")];

    assert_eq!(before[0], after[0], "the two differ only in address");
    assert!(!same_sources(&seen, &after));
}

#[test]
fn a_queue_that_gained_or_lost_a_row_is_not_the_same_sources() {
    let tracks = [summary(1, "Alpha"), summary(2, "Beta")];
    let seen = shadow(&tracks);

    assert!(!same_sources(&seen, &tracks[..1]));
    assert!(!same_sources(&seen[..1], &tracks));
}

#[test]
fn a_reordered_queue_is_not_the_same_sources() {
    let tracks = [summary(1, "Alpha"), summary(2, "Beta")];
    let seen = shadow(&tracks);
    let swapped = [Arc::clone(&tracks[1]), Arc::clone(&tracks[0])];

    assert!(!same_sources(&seen, &swapped));
}

/// The close teardown hands the summaries back, so a reopen has nothing to compare and owes a
/// full rebuild rather than a selection pass over rows it never built.
#[test]
fn a_shadow_the_teardown_emptied_is_not_the_same_sources() {
    let tracks = [summary(1, "Alpha")];
    let released = vec![ShadowEntry {
        id: 1,
        selected: false,
        source: None,
    }];

    assert!(!same_sources(&released, &tracks));
}

#[test]
fn a_row_leaves_its_foreign_keys_for_the_link_fetch_to_stamp() {
    let row = to_slint_queue_row(&summary(7, "Alpha"), false);

    assert_eq!((row.album_id, row.artist_id, row.genre_id), (0, 0, 0));
    assert_eq!(row.id, 7);
    assert!(!row.selected);
}

#[test]
fn a_row_with_nothing_to_say_renders_empty_rather_than_absent() {
    let row = to_slint_queue_row(&summary(1, "Alpha"), true);

    assert_eq!(row.artist.as_str(), "");
    assert_eq!(row.artwork_path.as_str(), "");
    assert_eq!(row.title.as_str(), "Alpha");
    assert!(row.selected);
}

/// A negative duration is nonsense a row still has to draw, and the unclamped arithmetic renders
/// it as `0:-1` — the minute and the second are floored independently.
#[test]
fn a_negative_duration_is_drawn_as_no_time_at_all() {
    let mut track = summary(1, "Alpha");
    Arc::make_mut(&mut track).duration_ms = -1_000;

    assert_eq!(to_slint_queue_row(&track, false).display_duration.as_str(), "0:00");
}
