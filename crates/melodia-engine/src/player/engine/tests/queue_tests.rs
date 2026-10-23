use std::collections::HashSet;
use std::sync::Arc;

use super::*;
use melodia_core::error::AppError;

/// A queue of the tracks `1..=count`, in that order.
fn make_queue(count: i64) -> QueueState {
    let mut q = QueueState::default();
    q.add_tracks(&(1..=count).collect::<Vec<i64>>());
    q
}

// ── add_tracks ──────────────────────────────────────────────────────

#[test]
fn add_tracks_to_empty_queue() {
    let mut q = QueueState::default();
    q.add_tracks(&[1, 2]);

    assert_eq!(q.tracks.len(), 2);
    assert_eq!(q.play_order, vec![0, 1]);
    assert_eq!(q.original_order, vec![0, 1]);
}

#[test]
fn add_tracks_appends_to_existing() {
    let mut q = make_queue(2);
    q.add_tracks(&[3]);

    assert_eq!(q.tracks.len(), 3);
    assert_eq!(q.play_order, vec![0, 1, 2]);
    assert_eq!(q.original_order, vec![0, 1, 2]);
}

#[test]
fn add_tracks_increments_version() {
    let mut q = QueueState::default();
    let v0 = q.version;
    q.add_tracks(&[1]);
    assert!(q.version > v0);
}

// ── insert_next ─────────────────────────────────────────────────────

#[test]
fn insert_next_after_current() {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    q.insert_next(&[10]);

    // Should insert at position 2 in play_order
    assert_eq!(q.play_order[2], 3); // new track index
    assert_eq!(q.tracks[3], 10);
}

#[test]
fn insert_next_when_no_current() {
    let mut q = make_queue(2);
    q.current_index = None;
    q.insert_next(&[10]);

    // Should insert at position 0
    assert_eq!(q.play_order[0], 2); // new track index
}

#[test]
fn insert_next_clamps_to_end() -> Result<(), AppError> {
    let mut q = make_queue(2);
    q.current_index = Some(10); // way past end
    q.insert_next(&[10]);

    // Should clamp insert position to play_order.len()
    let last =
        q.play_order.last().ok_or_else(|| AppError::Validation("play_order empty".into()))?;
    assert_eq!(*last, 2);
    Ok(())
}

// ── remove_at ───────────────────────────────────────────────────────

#[test]
fn remove_at_out_of_bounds() {
    let mut q = make_queue(2);
    assert!(!q.remove_at(5));
}

#[test]
fn remove_at_adjusts_current_when_before() {
    let mut q = make_queue(3);
    q.current_index = Some(2);
    q.remove_at(0);
    assert_eq!(q.current_index, Some(1));
}

#[test]
fn remove_at_no_change_when_after() {
    let mut q = make_queue(3);
    q.current_index = Some(0);
    q.remove_at(2);
    assert_eq!(q.current_index, Some(0));
}

#[test]
fn remove_at_current_clamps_to_len() {
    let mut q = make_queue(2);
    q.current_index = Some(1); // last item
    q.remove_at(1);
    // Queue has 1 item left, current_index should clamp to 0
    assert_eq!(q.current_index, Some(0));
}

#[test]
fn remove_at_last_item_sets_minus_one() {
    let mut q = make_queue(1);
    q.current_index = Some(0);
    q.remove_at(0);
    assert_eq!(q.current_index, None);
    assert!(q.play_order.is_empty());
}

#[test]
fn remove_at_increments_version() {
    let mut q = make_queue(2);
    let v0 = q.version;
    q.remove_at(0);
    assert!(q.version > v0);
}

// ── remove_batch ────────────────────────────────────────────────────

#[test]
fn remove_batch_multiple() {
    let mut q = make_queue(5);
    q.remove_batch(&[1, 3]);
    assert_eq!(q.play_order.len(), 3);
}

#[test]
fn remove_batch_deduplicates() {
    let mut q = make_queue(3);
    q.remove_batch(&[1, 1, 1]);
    assert_eq!(q.play_order.len(), 2);
}

#[test]
fn remove_batch_empty_is_noop() {
    let mut q = make_queue(3);
    let v0 = q.version;
    q.remove_batch(&[]);
    assert_eq!(q.version, v0);
    assert_eq!(q.play_order.len(), 3);
}

// ── move_rows ───────────────────────────────────────────────────────

#[test]
fn move_rows_out_of_bounds() {
    let mut q = make_queue(3);
    assert!(!q.move_rows(&[0], 10));
    assert!(!q.move_rows(&[10], 0));
}

#[test]
fn move_rows_forward_adjusts_current() {
    let mut q = make_queue(4);
    q.current_index = Some(1);
    // The row ahead of the current one moves past it, so the current one steps up.
    q.move_rows(&[0], 2);
    assert_eq!(q.current_index, Some(0));
}

#[test]
fn move_rows_backward_adjusts_current() {
    let mut q = make_queue(4);
    q.current_index = Some(1);
    // A row from behind lands ahead of the current one, so the current one steps down.
    q.move_rows(&[3], 0);
    assert_eq!(q.current_index, Some(2));
}

#[test]
fn move_current_track() {
    let mut q = make_queue(4);
    q.current_index = Some(1);
    q.move_rows(&[1], 3);
    assert_eq!(q.current_index, Some(3));
}

// ── clear ───────────────────────────────────────────────────────────

#[test]
fn clear_resets_all() {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    q.clear();

    assert!(q.tracks.is_empty());
    assert!(q.play_order.is_empty());
    assert!(q.original_order.is_empty());
    assert_eq!(q.current_index, None);
}

// ── skip_to_index ───────────────────────────────────────────────────

#[test]
fn skip_to_index_valid() {
    let mut q = make_queue(3);
    let track = q.skip_to_index(2);
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(2));
}

#[test]
fn skip_to_index_out_of_bounds() {
    let mut q = make_queue(3);
    assert!(q.skip_to_index(10).is_none());
}

// ── advance ─────────────────────────────────────────────────────────

#[test]
fn advance_off_stops_at_end() {
    let mut q = make_queue(2);
    q.current_index = Some(1);
    q.repeat_mode = RepeatMode::Off;
    assert!(q.advance().is_none());
}

#[test]
fn advance_all_wraps() {
    let mut q = make_queue(3);
    q.current_index = Some(2);
    q.repeat_mode = RepeatMode::All;
    let track = q.advance();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(0));
}

#[test]
fn advance_one_repeats_current() {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    q.repeat_mode = RepeatMode::One;
    let track = q.advance();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(1));
}

#[test]
fn advance_empty_queue() {
    let mut q = QueueState::default();
    assert!(q.advance().is_none());
}

// ── advance_skip ────────────────────────────────────────────────────

#[test]
fn advance_skip_one_advances_within_queue() {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    q.repeat_mode = RepeatMode::One;
    let track = q.advance_skip();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(2));
}

#[test]
fn advance_skip_one_wraps_at_end() {
    let mut q = make_queue(3);
    q.current_index = Some(2);
    q.repeat_mode = RepeatMode::One;
    let track = q.advance_skip();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(0));
}

#[test]
fn advance_skip_all_wraps() {
    let mut q = make_queue(3);
    q.current_index = Some(2);
    q.repeat_mode = RepeatMode::All;
    let track = q.advance_skip();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(0));
}

#[test]
fn advance_skip_off_stops_at_end() {
    let mut q = make_queue(2);
    q.current_index = Some(1);
    q.repeat_mode = RepeatMode::Off;
    assert!(q.advance_skip().is_none());
}

// ── set_repeat_mode ─────────────────────────────────────────────────

/// A panel re-sends the mode it already shows, and a bump there re-publishes the whole queue to
/// every subscriber for nothing.
#[test]
fn setting_the_repeat_mode_already_in_force_leaves_the_version_alone() {
    let mut q = make_queue(3);
    q.repeat_mode = RepeatMode::All;
    let before = q.version;

    q.set_repeat_mode(RepeatMode::All);

    assert_eq!(q.version, before);
}

/// `with_state_emit` gates the queue re-emit on the version, so a move that skipped the bump would
/// leave the transport bar's repeat button showing the mode it just left.
#[test]
fn a_new_repeat_mode_bumps_the_version() {
    let mut q = make_queue(3);
    let before = q.version;

    q.set_repeat_mode(RepeatMode::One);

    assert_ne!(q.version, before);
}

// ── previous ────────────────────────────────────────────────────────

#[test]
fn previous_goes_back() {
    let mut q = make_queue(3);
    q.current_index = Some(2);
    let track = q.previous();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(1));
}

#[test]
fn previous_all_wraps_to_last() {
    let mut q = make_queue(3);
    q.current_index = Some(0);
    q.repeat_mode = RepeatMode::All;
    let track = q.previous();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(2));
}

#[test]
fn previous_one_wraps_to_last() {
    let mut q = make_queue(3);
    q.current_index = Some(0);
    q.repeat_mode = RepeatMode::One;
    let track = q.previous();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(2));
}

#[test]
fn previous_off_stays_at_zero() {
    let mut q = make_queue(3);
    q.current_index = Some(0);
    q.repeat_mode = RepeatMode::Off;
    let track = q.previous();
    assert!(track.is_some());
    assert_eq!(q.current_index, Some(0));
}

// ── peek_next ───────────────────────────────────────────────────────

#[test]
fn peek_next_does_not_mutate() {
    let mut q = make_queue(3);
    q.current_index = Some(0);
    let version_before = q.version;
    let _ = q.peek_next();
    assert_eq!(q.current_index, Some(0));
    assert_eq!(q.version, version_before);
}

#[test]
fn peek_next_off_at_end_returns_none() {
    let mut q = make_queue(2);
    q.current_index = Some(1);
    q.repeat_mode = RepeatMode::Off;
    assert!(q.peek_next().is_none());
}

#[test]
fn peek_next_all_at_end_returns_first() -> Result<(), AppError> {
    let mut q = make_queue(3);
    q.current_index = Some(2);
    q.repeat_mode = RepeatMode::All;
    let track = q.peek_next().ok_or_else(|| AppError::Validation("peek_next None".into()))?;
    assert_eq!(track, 1);
    Ok(())
}

#[test]
fn peek_next_one_returns_current() -> Result<(), AppError> {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    q.repeat_mode = RepeatMode::One;
    let track = q.peek_next().ok_or_else(|| AppError::Validation("peek_next None".into()))?;
    assert_eq!(track, 2); // tracks[play_order[1]] = tracks[1], id=2
    Ok(())
}

// ── shuffle ─────────────────────────────────────────────────────────

#[test]
fn begin_shuffle_returns_len_and_current() {
    let mut q = make_queue(5);
    q.current_index = Some(2);
    let (len, current) = q.begin_shuffle();
    assert_eq!(len, 5);
    assert_eq!(current, Some(2));
}

#[test]
fn begin_shuffle_no_current() {
    let q = make_queue(3);
    // current_index is None by default after add_tracks (we didn't set it).
    let (len, current) = q.begin_shuffle();
    assert_eq!(len, 3);
    assert_eq!(current, None);
}

#[test]
fn apply_shuffle_order_reorders() {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    // Shuffle: put current (1) first, then 2, then 0
    q.apply_shuffle_order(&[1, 2, 0]);

    assert_eq!(q.play_order, vec![1, 2, 0]);
    assert_eq!(q.current_index, Some(0));
    assert!(q.shuffle_enabled);
}

#[test]
fn unshuffle_restores_original_order() {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    // Shuffle
    q.apply_shuffle_order(&[1, 2, 0]);
    // Current track (id=2) is now at play_order index 0
    q.unshuffle();

    assert_eq!(q.play_order, vec![0, 1, 2]);
    assert!(!q.shuffle_enabled);
    // Current track (id=2) should be at original position 1
    assert_eq!(q.current_index, Some(1));
}

// ── cycle_repeat_mode ───────────────────────────────────────────────

#[test]
fn cycle_repeat_mode_sequence() {
    let mut q = QueueState::default();
    assert_eq!(q.repeat_mode, RepeatMode::Off);
    q.cycle_repeat_mode();
    assert_eq!(q.repeat_mode, RepeatMode::All);
    q.cycle_repeat_mode();
    assert_eq!(q.repeat_mode, RepeatMode::One);
    q.cycle_repeat_mode();
    assert_eq!(q.repeat_mode, RepeatMode::Off);
}

// ── to_persistable ──────────────────────────────────────────────────

#[test]
fn to_persistable_maps_track_ids() {
    let mut q = make_queue(3);
    q.current_index = Some(1);

    let p = q.to_persistable();
    assert_eq!(p.track_ids, vec![1, 2, 3]);
    assert_eq!(p.current_index, 1);
}

// ── play_ids ────────────────────────────────────────────────────────

#[test]
fn play_ids_returns_correct_sequence() {
    let mut q = make_queue(3);
    // Reverse the play order
    q.play_order = vec![2, 0, 1];
    assert_eq!(*q.play_ids(), [3, 1, 2]);
}

/// What a row does to the queue.
type Edit = fn(&mut QueueState);

/// The ids `q` publishes once `act` has run, asked for first so a cache is held across it.
fn published_after(mut q: QueueState, act: Edit) -> Vec<i64> {
    let _held = q.play_ids();
    act(&mut q);
    q.play_ids().to_vec()
}

/// [`make_queue`]'s three tracks played in the order 3, 1, 2.
fn shuffled_three() -> QueueState {
    let mut q = make_queue(3);
    q.apply_shuffle_order(&[2, 0, 1]);
    q
}

/// Two tracks playing the second, which an anchored shuffle can only put first, whatever the draw.
fn on_the_second_of_two() -> QueueState {
    let mut q = make_queue(2);
    q.current_index = Some(1);
    q
}

/// The ids are cached until the order moves, so a mutator that kept them would publish an order
/// the queue no longer holds, to the queue panel, Up Next and `queue.json` alike.
#[test]
fn every_change_to_the_order_publishes_the_new_order() {
    let rows: [(&str, QueueState, Edit, &[i64]); 10] = [
        ("add_tracks", make_queue(3), |q| q.add_tracks(&[4]), &[1, 2, 3, 4]),
        ("insert_next", make_queue(3), |q| q.insert_next(&[4]), &[4, 1, 2, 3]),
        (
            "remove_at",
            make_queue(3),
            |q| {
                q.remove_at(0);
            },
            &[2, 3],
        ),
        ("remove_batch", make_queue(3), |q| q.remove_batch(&[0, 2]), &[2]),
        (
            "move_rows",
            make_queue(3),
            |q| {
                q.move_rows(&[0], 2);
            },
            &[2, 3, 1],
        ),
        ("clear", make_queue(3), QueueState::clear, &[]),
        ("apply_shuffle_order", make_queue(3), |q| q.apply_shuffle_order(&[2, 0, 1]), &[3, 1, 2]),
        (
            "shuffle_play_order_in_place",
            on_the_second_of_two(),
            |q| q.shuffle_play_order_in_place(&mut rand::rng(), true),
            &[2, 1],
        ),
        ("unshuffle", shuffled_three(), QueueState::unshuffle, &[1, 2, 3]),
        (
            "prune_missing",
            make_queue(3),
            |q| {
                q.prune_missing(&HashSet::from([2]));
            },
            &[1, 3],
        ),
    ];
    for (what, q, act, expected) in rows {
        assert_eq!(published_after(q, act), expected, "{what}");
    }
}

/// An advance moves only the index, so it republishes the allocation already shared rather than
/// building a library-sized array under the state lock on every track change.
#[test]
fn moving_the_index_alone_republishes_the_ids_already_shared() {
    let rows: [(&str, Edit); 6] = [
        ("advance", |q| {
            q.advance();
        }),
        ("advance_skip", |q| {
            q.advance_skip();
        }),
        ("previous", |q| {
            q.previous();
        }),
        ("skip_to_index", |q| {
            q.skip_to_index(2);
        }),
        ("set_repeat_mode", |q| q.set_repeat_mode(RepeatMode::All)),
        ("cycle_repeat_mode", QueueState::cycle_repeat_mode),
    ];
    for (what, act) in rows {
        let mut q = make_queue(3);
        q.current_index = Some(1);
        let before = q.play_ids();

        act(&mut q);

        assert!(Arc::ptr_eq(&before, &q.play_ids()), "{what} built the ids again");
    }
}

// ── current_id ──────────────────────────────────────────────────────

#[test]
fn current_id_returns_none_when_empty() {
    let q = QueueState::default();
    assert!(q.current_id().is_none());
}

#[test]
fn current_id_returns_none_for_negative_index() {
    let q = make_queue(3);
    // current_index is None by default
    assert!(q.current_id().is_none());
}

// ── prune_missing ───────────────────────────────────────────────────

#[test]
fn prune_missing_empty_set_is_noop() {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    let v0 = q.version;
    let outcome = q.prune_missing(&HashSet::new());

    assert_eq!(outcome.removed, 0);
    assert!(!outcome.current_was_removed);
    assert_eq!(q.tracks.len(), 3);
    assert_eq!(q.current_index, Some(1));
    assert_eq!(q.version, v0);
}

#[test]
fn prune_missing_no_matches_is_noop() {
    let mut q = make_queue(3);
    let v0 = q.version;
    let outcome = q.prune_missing(&HashSet::from([99, 100]));

    assert_eq!(outcome.removed, 0);
    assert_eq!(q.tracks.len(), 3);
    assert_eq!(q.version, v0);
}

#[test]
fn prune_missing_keeps_current_when_other_removed() -> Result<(), AppError> {
    // tracks 1..=5, current = play_order[2] = track id 3. Prune ids {1, 5}.
    // Expected: surviving tracks [2, 3, 4], current_index remapped to 1
    // (still pointing at id 3), `current_was_removed = false`.
    let mut q = make_queue(5);
    q.current_index = Some(2);

    let outcome = q.prune_missing(&HashSet::from([1, 5]));

    assert_eq!(outcome.removed, 2);
    assert!(!outcome.current_was_removed);
    assert_eq!(q.tracks, vec![2, 3, 4]);
    assert_eq!(q.play_order, vec![0, 1, 2]);
    assert_eq!(q.original_order, vec![0, 1, 2]);
    let current =
        q.current_id().ok_or_else(|| AppError::Validation("current_id None after prune".into()))?;
    assert_eq!(current, 3);
    Ok(())
}

#[test]
fn prune_missing_advances_when_current_removed() -> Result<(), AppError> {
    // tracks 1..=3, current = play_order[1] = id 2. Prune {2}.
    // Expected: surviving tracks [1, 3], current_index = 1 (next survivor,
    // which is id 3), `current_was_removed = true`.
    let mut q = make_queue(3);
    q.current_index = Some(1);

    let outcome = q.prune_missing(&HashSet::from([2]));

    assert!(outcome.current_was_removed);
    assert_eq!(outcome.removed, 1);
    assert_eq!(q.tracks, vec![1, 3]);
    let current =
        q.current_id().ok_or_else(|| AppError::Validation("current_id None after prune".into()))?;
    assert_eq!(current, 3);
    Ok(())
}

#[test]
fn prune_missing_clamps_to_last_survivor_when_tail_removed() -> Result<(), AppError> {
    // tracks 1..=4, current = 3 (id 4). Prune {3, 4}.
    // Expected: surviving [1, 2], no surviving slot at or after old current,
    // so we clamp to the last survivor (index 1, id 2). current_was_removed
    // is true.
    let mut q = make_queue(4);
    q.current_index = Some(3);

    let outcome = q.prune_missing(&HashSet::from([3, 4]));

    assert!(outcome.current_was_removed);
    assert_eq!(q.current_index, Some(1));
    let current =
        q.current_id().ok_or_else(|| AppError::Validation("current_id None after prune".into()))?;
    assert_eq!(current, 2);
    Ok(())
}

#[test]
fn prune_missing_everything_empties_queue() {
    let mut q = make_queue(3);
    q.current_index = Some(0);

    let outcome = q.prune_missing(&HashSet::from([1, 2, 3]));

    assert_eq!(outcome.removed, 3);
    assert!(outcome.current_was_removed);
    assert!(q.tracks.is_empty());
    assert!(q.play_order.is_empty());
    assert!(q.original_order.is_empty());
    assert_eq!(q.current_index, None);
}

#[test]
fn prune_missing_remaps_shuffled_order() {
    // tracks 1..=4 with a shuffled play_order. Pruning id 1 must drop the
    // play_order entry that pointed at it and remap every remaining entry
    // to the new tracks-vec position.
    let mut q = make_queue(4);
    // Hand-craft a shuffle: [t3, t1, t4, t2] in play order.
    q.play_order = vec![2, 0, 3, 1];
    q.shuffle_enabled = true;
    q.current_index = Some(0); // playing t3

    let outcome = q.prune_missing(&HashSet::from([1]));

    assert_eq!(outcome.removed, 1);
    assert!(!outcome.current_was_removed);
    // tracks after prune: [t2, t3, t4] → t3 at new idx 1, t4 at new idx 2, t2 at new idx 0.
    // Old play_order [2,0,3,1] -> drop entry 1 (was t1), remap remaining:
    //   2→1 (t3), 3→2 (t4), 1→0 (t2). New: [1, 2, 0].
    assert_eq!(q.play_order, vec![1, 2, 0]);
    // original_order had [0,1,2,3] → drop t1 (idx 0), remap survivors:
    //   1→0 (t2), 2→1 (t3), 3→2 (t4). New: [0, 1, 2].
    assert_eq!(q.original_order, vec![0, 1, 2]);
}

// ── current_id ──────────────────────────────────────────────────────

#[test]
fn current_id_returns_track() -> Result<(), AppError> {
    let mut q = make_queue(3);
    q.current_index = Some(1);
    let track = q.current_id().ok_or_else(|| AppError::Validation("current_id None".into()))?;
    assert_eq!(track, 2);
    Ok(())
}

// ── window_slots ────────────────────────────────────────────────────

/// The slot Previous lands on, the current one and the three a Next and an auto-skip run through,
/// cut at the ends of a queue that doesn't wrap. Ten slots, so only the edges under test cut it.
#[test]
fn the_window_runs_from_the_slot_before_the_current_one_to_three_after_it() {
    let rows: [(&str, usize, Option<usize>, &[usize]); 8] = [
        ("at the first slot", 10, Some(0), &[0, 1, 2, 3]),
        ("one slot in", 10, Some(1), &[0, 1, 2, 3, 4]),
        ("ending on the last slot", 10, Some(6), &[5, 6, 7, 8, 9]),
        ("one slot past the end", 10, Some(7), &[6, 7, 8, 9]),
        ("at the last slot", 10, Some(9), &[8, 9]),
        ("with nothing current", 10, None, &[0, 1, 2]),
        ("over a single track", 1, Some(0), &[0]),
        ("over an empty queue", 0, None, &[]),
    ];
    for (what, len, current, expected) in rows {
        let slots: Vec<usize> = window_slots(len, current, false).collect();

        assert_eq!(slots, expected, "{what}");
    }
}

/// Wrapping, the window carries on past either end as Previous and a manual Next do. A queue
/// shorter than the window names a slot twice, which `window_ids` folds.
#[test]
fn a_wrapping_window_carries_on_past_either_end_of_the_queue() {
    let rows: [(&str, usize, Option<usize>, &[usize]); 6] = [
        ("at the first slot", 10, Some(0), &[9, 0, 1, 2, 3]),
        ("at the last slot", 10, Some(9), &[8, 9, 0, 1, 2]),
        ("with nothing current", 10, None, &[8, 9, 0, 1, 2]),
        ("over a queue shorter than the window", 3, Some(0), &[2, 0, 1, 2, 0]),
        ("over a single track", 1, Some(0), &[0, 0, 0, 0, 0]),
        ("over an empty queue", 0, None, &[]),
    ];
    for (what, len, current, expected) in rows {
        let slots: Vec<usize> = window_slots(len, current, true).collect();

        assert_eq!(slots, expected, "{what}");
    }
}

// ── window_ids ──────────────────────────────────────────────────────

/// Repeat decides whether the window reaches past the last track and back past the first, as it
/// decides for Next and Previous. Repeat-one wraps too: its manual Next does.
#[test]
fn the_window_reaches_past_either_end_only_under_a_repeat() {
    let rows = [
        (RepeatMode::Off, Some(4), vec![4, 5]),
        (RepeatMode::All, Some(4), vec![4, 5, 1, 2, 3]),
        (RepeatMode::One, Some(4), vec![4, 5, 1, 2, 3]),
        (RepeatMode::Off, Some(0), vec![1, 2, 3, 4]),
        (RepeatMode::All, Some(0), vec![5, 1, 2, 3, 4]),
        (RepeatMode::One, Some(0), vec![5, 1, 2, 3, 4]),
    ];
    for (mode, current, expected) in rows {
        let mut q = make_queue(5);
        q.current_index = current;
        q.repeat_mode = mode;

        assert_eq!(q.window_ids(), expected, "{mode:?} at slot {current:?}");
    }
}

/// The window holds one record per track, so a track queued twice inside it is fetched once.
#[test]
fn a_track_queued_twice_inside_the_window_is_named_once() {
    let mut q = QueueState::default();
    q.add_tracks(&[1, 2, 1, 3]);
    q.current_index = Some(0);

    assert_eq!(q.window_ids(), vec![1, 2, 3]);
}

/// A transition moves through the play order, so a shuffled queue's window is the tracks around
/// the current one as they will play, not as they were added.
#[test]
fn the_window_follows_the_play_order() {
    let mut q = make_queue(5);
    q.apply_shuffle_order(&[4, 3, 2, 1, 0]);

    assert_eq!(q.window_ids(), vec![5, 4, 3, 2]);
}
