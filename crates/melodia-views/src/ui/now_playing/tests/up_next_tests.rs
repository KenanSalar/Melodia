//! The wrap arithmetic behind the Up Next list.
//!
//! Both functions here answer for a queue that is a *cycle* under repeat-all and a plain forward
//! slice under repeat-off, and every interesting case is a boundary: a queue of one, the last
//! index, and nothing playing at all.

use super::super::UP_NEXT_N;
use super::{
    SlideKind, UpNextAnchor, classify_step, outgoing_row, play_order_move, upcoming_id_slice,
    upcoming_indices,
};
use melodia_engine::player::engine::queue::{QueueState, current_index_to_i32};
use melodia_engine::player::engine::state::QueueViewModel;
use melodia_engine::player::engine::types::RepeatMode;
use melodia_ui::QueueRow;

fn queue(len: i64, index: i32, repeat_mode: RepeatMode) -> QueueViewModel {
    QueueViewModel {
        ids: (1..=len).collect(),
        queue_index: index,
        upcoming_from: usize::try_from(index + 1).unwrap_or(0),
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

/// `queue_index` is `-1` with nothing playing, so the base is zero and the whole queue is what
/// comes next, the cycle having no playing row to stop short of.
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
    let previous = UpNextAnchor::of(&queue(len, old_idx, RepeatMode::All));
    classify_step(previous, &queue(len, new_idx, RepeatMode::All)).direction()
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

/// The rows a list showing `ids` holds, which is where the slide reads its outgoing row from.
fn shown(ids: &[i32]) -> Vec<QueueRow> {
    ids.iter().map(|&id| QueueRow { id, ..QueueRow::default() }).collect()
}

#[test]
fn only_a_forward_step_carries_the_promoted_track_off_the_top() {
    let before = shown(&[2, 3, 4, 5]);

    let promoted = outgoing_row(SlideKind::Forward, &before, Some(2), None);
    assert_eq!(promoted.map(|row| row.id), Some(2));

    // Nothing to carry: the caller passed no id for this direction.
    assert!(outgoing_row(SlideKind::Forward, &before, None, Some(3)).is_none());
}

/// Backward carries the row that fell off the bottom, and only when it really fell off — the
/// caller answers that, so `None` there is not a step with nothing to show.
#[test]
fn a_backward_step_carries_the_row_that_fell_off_the_bottom() {
    let before = shown(&[3, 4]);

    assert_eq!(outgoing_row(SlideKind::Backward, &before, Some(2), Some(4)).map(|r| r.id), Some(4));
    assert!(outgoing_row(SlideKind::Backward, &before, Some(2), None).is_none());
}

#[test]
fn a_skip_carries_nothing_off_in_either_direction() {
    let before = shown(&[2, 3, 4]);

    assert!(outgoing_row(SlideKind::Other, &before, Some(2), Some(4)).is_none());
}

/// An id the list wasn't showing is nothing to render, which is what a rebuild that dropped the
/// track mid-transition leaves behind.
#[test]
fn an_id_the_list_was_not_showing_carries_nothing_off() {
    let before = shown(&[2, 3]);

    assert!(outgoing_row(SlideKind::Forward, &before, Some(99), None).is_none());
}

/// `(what, base, len, from_row, to_row, the play-order move)`.
type MoveCase = (&'static str, usize, usize, usize, usize, Option<(usize, usize)>);

/// `1..=6` playing slot 2 under repeat-all lists slots 3, 4 and 5, then wraps to 0 and 1: base 3,
/// with row 3 the boundary, `base + row == len`, and the first row past the wrap.
#[test]
fn a_move_between_up_next_rows_maps_onto_the_play_order_within_one_run() {
    let rows: [MoveCase; 11] = [
        ("within the run before the wrap", 3, 6, 0, 2, Some((3, 5))),
        ("within the run after the wrap", 3, 6, 4, 3, Some((1, 0))),
        ("from the boundary row within its run", 3, 6, 3, 4, Some((0, 1))),
        ("across the wrap from before it", 3, 6, 0, 4, None),
        ("across the wrap from after it", 3, 6, 4, 0, None),
        ("from the boundary row across the wrap", 3, 6, 3, 2, None),
        ("onto the boundary row across the wrap", 3, 6, 2, 3, None),
        ("down a queue with no repeat", 2, 6, 0, 3, Some((2, 5))),
        ("up a queue with no repeat", 2, 6, 3, 0, Some((5, 2))),
        ("with nothing playing", 0, 4, 3, 0, Some((3, 0))),
        ("in an empty queue", 0, 0, 0, 0, None),
    ];
    for (what, base, len, from_row, to_row, expected) in rows {
        assert_eq!(play_order_move(base, len, from_row, to_row), expected, "{what}");
    }
}

/// The Now Playing view's projection of `q`.
fn view_of(q: &QueueState) -> QueueViewModel {
    QueueViewModel {
        ids: q.play_ids(),
        queue_index: current_index_to_i32(q.playing_index()),
        upcoming_from: q.upcoming_from(),
        shuffle_enabled: q.shuffle_enabled,
        repeat_mode: q.repeat_mode,
        has_next: true,
        has_previous: true,
    }
}

/// Up Next once `from_row` is dropped on `to_row` over `1..=6` playing track 3 under repeat-all,
/// through the mapping and then the engine's own move. `None` where the mapping refused.
fn up_next_after(from_row: usize, to_row: usize) -> Option<Vec<i64>> {
    let mut q = QueueState::default();
    q.add_tracks(&[1, 2, 3, 4, 5, 6]);
    q.current_index = Some(2);
    q.set_repeat_mode(RepeatMode::All);
    let view = view_of(&q);
    let (_, base) = upcoming_indices(&view);

    let (from, to) = play_order_move(base, view.ids.len(), from_row, to_row)?;
    q.move_rows(&[from], to);

    Some(upcoming_id_slice(&view_of(&q)))
}

/// The list moves its own row with `remove` and `insert` before the queue answers, so the mapping
/// and the engine's move have to agree on counting `to` once the row has left, or the publish that
/// follows puts the row one off from where it was dropped. Up Next starts as 4, 5, 6, 1, 2.
#[test]
fn a_mapped_move_leaves_up_next_as_the_drop_left_the_shown_rows() {
    let rows: [(usize, usize, [i64; 5]); 4] = [
        (0, 2, [5, 6, 4, 1, 2]),
        (2, 0, [6, 4, 5, 1, 2]),
        (4, 3, [4, 5, 6, 2, 1]),
        (3, 4, [4, 5, 6, 2, 1]),
    ];
    for (from_row, to_row, expected) in rows {
        assert_eq!(
            up_next_after(from_row, to_row),
            Some(expected.to_vec()),
            "row {from_row} to row {to_row}"
        );
    }
}
