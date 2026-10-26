//! The queue's entries, which tell two copies of a track apart and let a caller that awaits between
//! reading the queue and editing it find its row again.

use std::collections::HashSet;

use super::*;
use melodia_core::error::AppError;

/// What a row does to the queue.
type Edit = fn(&mut QueueState);

/// A queue of `ids` in their own order, nothing playing.
fn queue_of(ids: &[i64]) -> QueueState {
    let mut q = QueueState::default();
    q.add_tracks(ids);
    q
}

fn five() -> QueueState {
    queue_of(&[1, 2, 3, 4, 5])
}

/// [`five`] playing in reverse.
fn reversed_five() -> QueueState {
    let mut q = five();
    q.apply_shuffle_order(&[4, 3, 2, 1, 0]);
    q
}

/// The entry on `row`, failing where there is none so a fixture can't hand a test a `None` it
/// would pass on.
fn entry_on(q: &QueueState, row: usize) -> Result<QueueEntry, AppError> {
    q.entry_at(row).ok_or_else(|| AppError::validation_msg(format!("no entry on row {row}")))
}

/// The entry on the row holding `id`, for a queue holding it once.
fn entry_holding(q: &QueueState, id: i64) -> Result<QueueEntry, AppError> {
    let row = q.play_ids().iter().position(|&queued| queued == id);
    row.and_then(|row| q.entry_at(row))
        .ok_or_else(|| AppError::validation_msg(format!("track {id} is not queued")))
}

/// The row `entry` sits on, `None` once it has left.
fn row_of(q: &QueueState, entry: QueueEntry) -> Option<usize> {
    q.play_entries().iter().position(|&queued| queued == entry)
}

fn current_entry(q: &QueueState) -> Option<QueueEntry> {
    q.current_index.and_then(|ci| q.entry_at(ci))
}

// ── entries ─────────────────────────────────────────────────────────

#[test]
fn a_track_queued_twice_gets_two_entries() {
    let q = queue_of(&[1, 2, 1]);

    assert_ne!(q.entry_at(0), q.entry_at(2));
}

/// The second copy of track 1 in `1, 2, 1, 3`, followed through every edit that keeps its row.
#[test]
fn an_entry_stays_on_its_row_through_every_edit_that_keeps_it() -> Result<(), AppError> {
    let rows: [(&str, Edit, usize); 6] = [
        (
            "a move of another row past it",
            |q| {
                q.move_rows(&[0], 3);
            },
            1,
        ),
        ("a shuffle", |q| q.apply_shuffle_order(&[2, 3, 0, 1]), 0),
        (
            "a shuffle and an unshuffle",
            |q| {
                q.apply_shuffle_order(&[2, 3, 0, 1]);
                q.unshuffle();
            },
            2,
        ),
        (
            "a removal of a row above it",
            |q| {
                q.remove_at(0);
            },
            1,
        ),
        ("a removal of other rows either side", |q| q.remove_batch(&[1, 3]), 1),
        (
            "a prune of another track",
            |q| {
                q.prune_missing(&HashSet::from([2]));
            },
            1,
        ),
    ];
    for (what, edit, expected) in rows {
        let mut q = queue_of(&[1, 2, 1, 3]);
        let followed = entry_on(&q, 2)?;

        edit(&mut q);

        assert_eq!(row_of(&q, followed), Some(expected), "{what}");
    }
    Ok(())
}

/// A pick or a drop still holding an entry from before a clear would otherwise land on whatever
/// the refill put in that slot.
#[test]
fn a_clear_never_hands_an_entry_out_again() {
    let mut q = queue_of(&[1, 2]);
    let before = q.play_entries();

    q.clear();
    q.add_tracks(&[1, 2]);

    let after = q.play_entries();
    let reused: Vec<&QueueEntry> = after.iter().filter(|entry| before.contains(entry)).collect();
    assert_eq!(reused, Vec::<&QueueEntry>::new());
}

// ── insert_next and insert_before ───────────────────────────────────

/// `1, 2, 3, 4` playing track 3, taking a block of 10 and 11. The current row's index moves only
/// for a block landing at or before it, which is what keeps the same track playing.
#[test]
fn a_block_moves_the_current_index_only_when_it_lands_at_or_before_the_current_row() {
    let rows: [(&str, Edit, [i64; 6], usize); 5] = [
        (
            "dropped before it",
            |q| q.insert_before(q.entry_at(1), &[10, 11]),
            [1, 10, 11, 2, 3, 4],
            4,
        ),
        ("dropped on it", |q| q.insert_before(q.entry_at(2), &[10, 11]), [1, 2, 10, 11, 3, 4], 4),
        (
            "dropped after it",
            |q| q.insert_before(q.entry_at(3), &[10, 11]),
            [1, 2, 3, 10, 11, 4],
            2,
        ),
        ("played next", |q| q.insert_next(&[10, 11]), [1, 2, 3, 10, 11, 4], 2),
        ("dropped onto no row", |q| q.insert_before(None, &[10, 11]), [1, 2, 3, 4, 10, 11], 2),
    ];
    for (what, edit, expected, current) in rows {
        let mut q = queue_of(&[1, 2, 3, 4]);
        q.current_index = Some(2);

        edit(&mut q);

        assert_eq!(
            (q.current_id(), q.current_index, q.play_ids().to_vec()),
            (Some(3), Some(current), expected.to_vec()),
            "{what}"
        );
    }
}

#[test]
fn a_block_with_nothing_current_lands_at_the_front_or_fills_an_empty_queue() {
    let rows: [(&str, QueueState, Edit, &[i64]); 3] = [
        (
            "played next with nothing current",
            queue_of(&[1, 2, 3]),
            |q| q.insert_next(&[10, 11]),
            &[10, 11, 1, 2, 3],
        ),
        (
            "played next into an empty queue",
            QueueState::default(),
            |q| q.insert_next(&[10, 11]),
            &[10, 11],
        ),
        (
            "dropped into an empty queue",
            QueueState::default(),
            |q| q.insert_before(None, &[10, 11]),
            &[10, 11],
        ),
    ];
    for (what, mut q, edit, expected) in rows {
        edit(&mut q);

        assert_eq!((q.current_index, q.play_ids().to_vec()), (None, expected.to_vec()), "{what}");
    }
}

/// The version gates the queue's re-publish, so a bump here would repaint every list showing the
/// queue for a drop that carried nothing.
#[test]
fn an_empty_block_changes_nothing_the_version_included() {
    let rows: [(&str, Edit); 3] = [
        ("played next", |q| q.insert_next(&[])),
        ("dropped on a row", |q| q.insert_before(q.entry_at(1), &[])),
        ("dropped onto no row", |q| q.insert_before(None, &[])),
    ];
    for (what, edit) in rows {
        let mut q = queue_of(&[1, 2, 3]);
        q.current_index = Some(1);
        let before = (q.version, q.current_index, q.play_ids().to_vec());

        edit(&mut q);

        assert_eq!((q.version, q.current_index, q.play_ids().to_vec()), before, "{what}");
    }
}

/// The insertion order takes a block at its end, as an append does, so an unshuffle can't scatter
/// a Play Next of an album through the tracks it was queued among.
#[test]
fn a_block_played_next_under_shuffle_unshuffles_in_the_order_handed_over() {
    let mut q = queue_of(&[1, 2, 3, 4]);
    q.apply_shuffle_order(&[2, 0, 3, 1]);
    q.insert_next(&[10, 11, 12]);

    q.unshuffle();

    assert_eq!(*q.play_ids(), [1, 2, 3, 4, 10, 11, 12]);
}

// ── a drop after the queue moved ────────────────────────────────────

/// What `q` holds once `edit` has run between a drop reading the row holding track 4 and the drop
/// landing 10 and 11 ahead of it, the library awaiting between the two.
fn dropped_ahead_of_four(mut q: QueueState, edit: Edit) -> Result<Vec<i64>, AppError> {
    let target = entry_holding(&q, 4)?;
    edit(&mut q);
    q.insert_before(Some(target), &[10, 11]);
    Ok(q.play_ids().to_vec())
}

#[test]
fn a_drop_lands_directly_ahead_of_its_row_however_the_queue_moved_first() -> Result<(), AppError> {
    let rows: [(&str, QueueState, Edit, &[i64]); 7] = [
        (
            "a removal above it",
            five(),
            |q| {
                q.remove_at(0);
            },
            &[2, 3, 10, 11, 4, 5],
        ),
        (
            "a prune of another track",
            five(),
            |q| {
                q.prune_missing(&HashSet::from([2]));
            },
            &[1, 3, 10, 11, 4, 5],
        ),
        (
            "a move of its own row",
            five(),
            |q| {
                q.move_rows(&[3], 0);
            },
            &[10, 11, 4, 1, 2, 3, 5],
        ),
        (
            "a shuffle",
            five(),
            |q| q.apply_shuffle_order(&[4, 3, 2, 1, 0]),
            &[5, 10, 11, 4, 3, 2, 1],
        ),
        ("an unshuffle", reversed_five(), QueueState::unshuffle, &[1, 2, 3, 10, 11, 4, 5]),
        ("a Play Next", five(), |q| q.insert_next(&[20]), &[20, 1, 2, 3, 10, 11, 4, 5]),
        (
            "a second drop landing first",
            five(),
            |q| q.insert_before(q.entry_at(3), &[20]),
            &[1, 2, 3, 20, 10, 11, 4, 5],
        ),
    ];
    for (what, start, edit, expected) in rows {
        assert_eq!(dropped_ahead_of_four(start, edit)?, expected, "{what}");
    }
    Ok(())
}

/// A row that left names no place, so the block goes where an append would rather than wherever
/// the slot the row used to hold now sits.
#[test]
fn a_drop_whose_row_has_left_lands_at_the_end() -> Result<(), AppError> {
    let rows: [(&str, Edit, &[i64]); 3] = [
        (
            "its row removed",
            |q| {
                q.remove_at(3);
            },
            &[1, 2, 3, 5, 10, 11],
        ),
        (
            "its track pruned",
            |q| {
                q.prune_missing(&HashSet::from([4]));
            },
            &[1, 2, 3, 5, 10, 11],
        ),
        (
            "the queue cleared and refilled",
            |q| {
                q.clear();
                q.add_tracks(&[1, 2, 3, 4, 5]);
            },
            &[1, 2, 3, 4, 5, 10, 11],
        ),
    ];
    for (what, edit, expected) in rows {
        assert_eq!(dropped_ahead_of_four(five(), edit)?, expected, "{what}");
    }
    Ok(())
}

// ── skip_to_entry ───────────────────────────────────────────────────

/// What a click on the row holding track 4 plays, and where it leaves the current index, once
/// `edit` has run between the click reading the row and the skip.
fn skipped_to_four(edit: Edit) -> Result<(Option<i64>, Option<usize>), AppError> {
    let mut q = five();
    let clicked = entry_holding(&q, 4)?;
    edit(&mut q);
    let played = q.skip_to_entry(clicked);
    Ok((played, q.current_index))
}

#[test]
fn a_skip_plays_the_row_clicked_wherever_the_queue_has_moved_it() -> Result<(), AppError> {
    let rows: [(&str, Edit, usize); 4] = [
        ("a Play Next above it", |q| q.insert_next(&[20]), 4),
        ("a drop above it", |q| q.insert_before(q.entry_at(0), &[20, 21]), 5),
        (
            "a move of its own row",
            |q| {
                q.move_rows(&[3], 0);
            },
            0,
        ),
        (
            "a move of another row past it",
            |q| {
                q.move_rows(&[0], 4);
            },
            2,
        ),
    ];
    for (what, edit, expected) in rows {
        assert_eq!(skipped_to_four(edit)?, (Some(4), Some(expected)), "{what}");
    }
    Ok(())
}

/// A click on a row that left while the skip was on its way plays nothing, rather than whatever
/// took its place.
#[test]
fn a_skip_to_a_row_that_has_left_plays_nothing() -> Result<(), AppError> {
    let rows: [(&str, Edit); 3] = [
        ("its row removed", |q| {
            q.remove_at(3);
        }),
        ("its track pruned", |q| {
            q.prune_missing(&HashSet::from([4]));
        }),
        ("the queue cleared and refilled", |q| {
            q.clear();
            q.add_tracks(&[1, 2, 3, 4, 5]);
        }),
    ];
    for (what, edit) in rows {
        assert_eq!(skipped_to_four(edit)?, (None, None), "{what}");
    }
    Ok(())
}

/// Finding the clicked row by id would play the first copy wherever the click landed.
#[test]
fn a_skip_to_the_second_copy_of_a_track_plays_that_copy() -> Result<(), AppError> {
    let mut q = queue_of(&[1, 2, 1, 3]);
    let second_copy = entry_on(&q, 2)?;
    q.insert_next(&[20]);

    let played = q.skip_to_entry(second_copy);

    assert_eq!((played, q.current_index), (Some(1), Some(3)));
    Ok(())
}

/// `queue_skip_to_index` returns before the fetch on this answer, so a click past the end of a
/// queue that shrank under it must name no row.
#[test]
fn the_row_past_the_end_names_no_entry() {
    let q = five();

    assert_eq!((q.entry_at(4).is_some(), q.entry_at(5)), (true, None));
}

// ── block moves round the current row ───────────────────────────────

/// `3, 1, 2, 3, 4, 5` playing the second copy of 3, so finding the current row again by its id
/// would land on the first copy.
#[test]
fn the_current_row_stays_current_wherever_a_block_moves() -> Result<(), AppError> {
    let rows: [(&str, &[usize], usize, usize); 6] = [
        ("a block holding it", &[2, 3], 4, 5),
        ("a scattered block holding it", &[1, 3, 5], 3, 4),
        ("a block passing it downward", &[1, 2], 3, 1),
        ("a block passing it upward", &[4, 5], 1, 5),
        ("a block landing right before it", &[4, 5], 3, 5),
        ("a block landing right after it", &[1, 2], 2, 1),
    ];
    for (what, block, to, expected) in rows {
        let mut q = queue_of(&[3, 1, 2, 3, 4, 5]);
        q.current_index = Some(3);
        let playing = entry_on(&q, 3)?;

        q.move_rows(block, to);

        assert_eq!((q.current_index, current_entry(&q)), (Some(expected), Some(playing)), "{what}");
    }
    Ok(())
}

/// Finding the current row again by id took the first copy, so an unshuffle while the second copy
/// played moved the current row onto the first, and Next carried on from there.
#[test]
fn an_unshuffle_keeps_the_playing_copy_of_a_track_queued_twice_current() -> Result<(), AppError> {
    let mut q = queue_of(&[1, 2, 1, 3]);
    q.current_index = Some(2);
    let playing = entry_on(&q, 2)?;
    q.apply_shuffle_order(&[2, 0, 1, 3]);

    q.unshuffle();

    assert_eq!((q.current_index, current_entry(&q)), (Some(2), Some(playing)));
    Ok(())
}
