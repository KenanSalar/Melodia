//! The wrap arithmetic behind the Up Next list.
//!
//! Both functions here answer for a queue that is a *cycle* under repeat-all and a plain forward
//! slice under repeat-off, and every interesting case is a boundary: a queue of one, the last
//! index, and nothing playing at all.

use std::sync::Arc;

use super::super::UP_NEXT_N;
use super::{SlideKind, classify_step, outgoing_row, upcoming_indices};
use melodia_core::entities::track::TrackSummary;
use melodia_engine::player::engine::state::QueueViewModel;
use melodia_engine::player::engine::types::RepeatMode;

fn summary(id: i64) -> Arc<TrackSummary> {
    Arc::new(TrackSummary {
        id,
        file_path: String::new(),
        file_name: String::new(),
        title: format!("Track {id}"),
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

fn queue(len: i64, index: i32, repeat_mode: RepeatMode) -> QueueViewModel {
    QueueViewModel {
        queue_tracks: (1..=len).map(summary).collect(),
        queue_index: index,
        shuffle_enabled: false,
        repeat_mode,
        has_next: true,
        has_previous: true,
    }
}

fn upcoming(len: i64, index: i32, repeat_mode: RepeatMode) -> (Vec<usize>, usize) {
    upcoming_indices(&queue(len, index, repeat_mode))
}

#[test]
fn repeat_off_takes_a_plain_forward_slice_and_stops_at_the_end() {
    assert_eq!(upcoming(5, 0, RepeatMode::Off), (vec![1, 2, 3, 4], 1));
    assert_eq!(upcoming(5, 3, RepeatMode::Off), (vec![4], 4));
    assert_eq!(upcoming(5, 4, RepeatMode::Off), (Vec::new(), 5));
}

/// The cycle stops just before the current track again, so the list never shows what is playing as
/// something still to come.
#[test]
fn a_repeating_queue_wraps_past_the_end_and_stops_short_of_the_current_track() {
    assert_eq!(upcoming(5, 3, RepeatMode::All), (vec![4, 0, 1, 2], 4));
    assert_eq!(upcoming(5, 4, RepeatMode::All), (vec![0, 1, 2, 3], 5));
    assert_eq!(upcoming(5, 0, RepeatMode::One), (vec![1, 2, 3, 4], 1));
}

/// A queue of one has nothing after it whichever way it repeats, and the wrap arm's `len - 1` is
/// what keeps that from becoming an endless list of the track already playing.
#[test]
fn a_queue_of_one_has_nothing_up_next() {
    assert_eq!(upcoming(1, 0, RepeatMode::All), (Vec::new(), 1));
    assert_eq!(upcoming(1, 0, RepeatMode::Off), (Vec::new(), 1));
}

/// `queue_index` is `-1` with nothing playing, so the base clamps to zero and the whole queue is
/// what comes next — and the wrap arm is off, there being no current track to stop before.
#[test]
fn nothing_playing_lists_the_queue_from_its_head() {
    assert_eq!(upcoming(3, -1, RepeatMode::All), (vec![0, 1, 2], 0));
    assert_eq!(upcoming(3, -1, RepeatMode::Off), (vec![0, 1, 2], 0));
}

#[test]
fn an_empty_queue_has_nothing_up_next() {
    assert_eq!(upcoming(0, -1, RepeatMode::All), (Vec::new(), 0));
    assert_eq!(upcoming(0, 0, RepeatMode::Off), (Vec::new(), 1));
}

/// The list is capped, and the cap is what a wrapped queue longer than it must still respect.
#[test]
fn a_queue_longer_than_the_list_is_cut_to_the_cap() {
    let long = i64::try_from(UP_NEXT_N).unwrap_or(i64::MAX) + 5;

    let (off, _) = upcoming(long, 0, RepeatMode::Off);
    assert_eq!(off.len(), UP_NEXT_N);

    let (wrapped, _) = upcoming(long, 0, RepeatMode::All);
    assert_eq!(wrapped.len(), UP_NEXT_N);
}

fn step(len: i64, old_idx: i32, new_idx: i32) -> i32 {
    classify_step(old_idx, &queue(len, new_idx, RepeatMode::All)).direction()
}

/// A single step is recognised **with wrap**, which is the whole reason repeat-all navigation
/// animates the right way rather than reading as a skip.
#[test]
fn a_single_step_is_recognised_in_both_directions_including_the_wrap() {
    assert_eq!(step(5, 0, 1), 1);
    assert_eq!(step(5, 4, 0), 1, "last to first is still a forward step");
    assert_eq!(step(5, 3, 2), -1);
    assert_eq!(step(5, 0, 4), -1, "first to last is still a backward step");
}

/// Anything else animates forward with no outgoing row: a skip-to, a rebuild, and every queue too
/// short or too empty to have a step at all.
#[test]
fn anything_that_is_not_a_step_animates_forward() {
    assert_eq!(step(5, 0, 3), 1, "a skip-to");
    assert_eq!(step(0, 0, 0), 1, "an empty queue");
    assert_eq!(step(5, -1, 0), 1, "nothing was playing");
    assert_eq!(step(5, 0, -1), 1, "nothing is playing now");
}

/// A queue of one steps to itself both ways, and forward wins — the row does not slide backwards
/// out of a list it is the whole of.
#[test]
fn a_queue_of_one_stepping_to_itself_reads_as_forward() {
    assert_eq!(step(1, 0, 0), 1);
}

#[test]
fn only_a_forward_step_carries_the_promoted_track_off_the_top() {
    let qvm = queue(5, 1, RepeatMode::All);

    let promoted = outgoing_row(SlideKind::Forward, &qvm, Some(2), None);
    assert_eq!(promoted.map(|row| row.id), Some(2));

    // Nothing to carry: the caller passed no id for this direction.
    assert!(outgoing_row(SlideKind::Forward, &qvm, None, Some(3)).is_none());
}

/// Backward carries the row that fell off the bottom, and only when it really fell off — the
/// caller answers that, so `None` there is not a step with nothing to show.
#[test]
fn a_backward_step_carries_the_row_that_fell_off_the_bottom() {
    let qvm = queue(5, 1, RepeatMode::All);

    assert_eq!(outgoing_row(SlideKind::Backward, &qvm, Some(2), Some(4)).map(|r| r.id), Some(4));
    assert!(outgoing_row(SlideKind::Backward, &qvm, Some(2), None).is_none());
}

#[test]
fn a_skip_carries_nothing_off_in_either_direction() {
    let qvm = queue(5, 1, RepeatMode::All);

    assert!(outgoing_row(SlideKind::Other, &qvm, Some(2), Some(4)).is_none());
}

/// An id the queue no longer holds is nothing to render, which is what a rebuild that dropped the
/// track mid-transition leaves behind.
#[test]
fn an_id_the_queue_has_dropped_carries_nothing_off() {
    let qvm = queue(5, 1, RepeatMode::All);

    assert!(outgoing_row(SlideKind::Forward, &qvm, Some(99), None).is_none());
}
