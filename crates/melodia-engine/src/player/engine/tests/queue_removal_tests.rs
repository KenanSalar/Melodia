//! What the queue's removals hold and how their undos land: the rows in both orders, and the
//! cursor, which goes back only while nothing has started since.

use crate::player::engine::fixtures::{cut_rows, queue_of};
use crate::player::engine::queue::{QueueCut, QueueEntry, QueueState};
use crate::player::engine::types::RepeatMode;
use melodia_core::error::AppError;

const FIVE: [i64; 5] = [1, 2, 3, 4, 5];

/// What a play replaces the queue with.
const PLAYED: [i64; 3] = [1, 2, 3];

/// What a moment of the queue answers to.
type Step = fn(&mut QueueState) -> Option<i64>;

/// A step, and the track it answers with the row lit after it.
type StepRow = (&'static str, Step, (Option<i64>, Option<usize>));

/// A queue as some history of edits left it.
type Built = fn() -> QueueState;

/// [`Built`], for a history whose cut can fail.
type TryBuilt = fn() -> Result<QueueState, AppError>;

/// Removes the row an entry names and undoes the removal.
type RemoveAndUndo = fn(&mut QueueState, QueueEntry);

/// A row moved past a gap: where the gap is, the rows moved, where to, and what plays next.
type MoveRow = (&'static str, usize, &'static [usize], usize, Option<i64>);

/// [`FIVE`] with the row at `row` playing.
fn five_playing(row: usize) -> QueueState {
    let mut q = queue_of(&FIVE);
    q.seat(row);
    q
}

/// [`five_playing`] with that row cut, so its track plays on with its row gone.
fn gapped_at(row: usize) -> Result<QueueState, AppError> {
    let mut q = five_playing(row);
    cut(&mut q, &[row])?;
    Ok(q)
}

/// Cuts `rows`, failing where nothing was taken so a test can't restore a `None` it would pass on.
fn cut(q: &mut QueueState, rows: &[usize]) -> Result<QueueCut, AppError> {
    cut_rows(q, rows).ok_or_else(|| AppError::validation_msg(format!("rows {rows:?} took nothing")))
}

/// The queue as the panel draws it: the ids in play order, and the row lit as playing.
fn shown(q: &QueueState) -> (Vec<i64>, Option<usize>) {
    (q.play_ids().to_vec(), q.playing_index())
}

// ── cut and restore_cuts ────────────────────────────────────────────

#[test]
fn an_undone_cut_puts_back_the_rows_and_the_row_lit_as_playing() -> Result<(), AppError> {
    let rows: [(&str, usize, &[usize]); 3] = [
        ("the playing row before the cut", 0, &[2, 3]),
        ("the playing row inside the cut", 2, &[1, 2]),
        ("the playing row after the cut", 4, &[1, 2]),
    ];
    for (what, playing, cut_at) in rows {
        let mut q = five_playing(playing);
        let before = shown(&q);
        let held = cut(&mut q, cut_at)?;

        q.restore_cuts(vec![held]);

        assert_eq!(shown(&q), before, "{what}");
    }
    Ok(())
}

/// The insertion order is what an unshuffle rebuilds the play order from, so a row restored to
/// the play order alone would come back somewhere else the moment shuffle goes off.
#[test]
fn an_undone_cut_under_shuffle_puts_back_both_orders() -> Result<(), AppError> {
    let mut q = queue_of(&FIVE);
    q.apply_shuffle_order(&[4, 2, 0, 3, 1]);
    let held = cut(&mut q, &[1, 3])?;

    q.restore_cuts(vec![held]);

    let shuffled = q.play_ids().to_vec();
    q.unshuffle();
    assert_eq!((shuffled, q.play_ids().to_vec()), (vec![5, 3, 1, 4, 2], FIVE.to_vec()));
    Ok(())
}

/// Undone oldest first, the first row back shifts the second's index by one.
#[test]
fn a_run_of_cuts_undone_together_puts_the_queue_back_whole() -> Result<(), AppError> {
    let mut q = five_playing(0);
    let first = cut(&mut q, &[1])?;
    let second = cut(&mut q, &[2])?;

    q.restore_cuts(vec![first, second]);

    assert_eq!(shown(&q), (FIVE.to_vec(), Some(0)));
    Ok(())
}

#[test]
fn a_cut_made_shuffled_comes_back_in_insertion_order_after_an_unshuffle() -> Result<(), AppError> {
    let mut q = queue_of(&FIVE);
    q.apply_shuffle_order(&[4, 2, 0, 3, 1]);
    let held = cut(&mut q, &[0])?;
    q.unshuffle();

    q.restore_cuts(vec![held]);

    assert_eq!(q.play_ids().to_vec(), FIVE);
    Ok(())
}

#[test]
fn an_undo_after_the_next_row_started_keeps_that_row_playing() -> Result<(), AppError> {
    let mut q = five_playing(2);
    let held = cut(&mut q, &[2])?;
    q.advance();

    q.restore_cuts(vec![held]);

    assert_eq!((q.current_id(), q.playing_index()), (Some(4), Some(3)));
    Ok(())
}

/// `close_gap` counts no seat, so nothing has started since the cut.
#[test]
fn an_undo_after_the_gap_closed_with_nothing_seated_lights_the_removed_row() -> Result<(), AppError>
{
    let mut q = five_playing(2);
    let held = cut(&mut q, &[2])?;
    q.close_gap();

    q.restore_cuts(vec![held]);

    assert_eq!((q.current_id(), q.playing_index()), (Some(3), Some(2)));
    Ok(())
}

/// A click from a panel drawn before the removal can't reach the row its undo put back.
#[test]
fn an_entry_a_removal_took_names_nothing_once_its_row_is_back() -> Result<(), AppError> {
    let rows: [(&str, RemoveAndUndo); 2] = [
        ("a cut", |q, entry| {
            let held = q.cut(&[entry]).into_iter().collect();
            q.restore_cuts(held);
        }),
        ("a clear", |q, _| {
            let held = q.take_cleared().into_iter().collect();
            q.restore_cleared(held);
        }),
    ];
    for (what, remove_and_undo) in rows {
        let mut q = queue_of(&FIVE);
        let entry = q.entry_at(1).ok_or_else(|| AppError::validation_msg("row 1 is queued"))?;
        remove_and_undo(&mut q, entry);

        assert_eq!(q.skip_to_entry(entry), None, "{what}");
    }
    Ok(())
}

// ── the gap a removed playing row leaves ────────────────────────────

/// The lit row is asserted beside the id: under Repeat One the id is the same whether the step
/// started the row that took the place or left the gap open.
#[test]
fn the_row_that_took_a_removed_playing_rows_place_plays_next() -> Result<(), AppError> {
    let rows: [StepRow; 6] = [
        ("the track's end", QueueState::advance, (Some(3), Some(1))),
        ("the gapless preload, which starts nothing", |q| q.peek_next(), (Some(3), None)),
        ("Next", QueueState::advance_skip, (Some(3), Some(1))),
        ("Previous", QueueState::previous, (Some(1), Some(0))),
        (
            "the track's end under Repeat One",
            |q| {
                q.set_repeat_mode(RepeatMode::One);
                q.advance()
            },
            (Some(3), Some(1)),
        ),
        (
            "Play Next, then the track's end",
            |q| {
                q.insert_next(&[9]);
                q.advance()
            },
            (Some(9), Some(1)),
        ),
    ];
    for (what, step, expected) in rows {
        let mut q = gapped_at(1)?;

        let played = step(&mut q);

        assert_eq!((played, q.playing_index()), expected, "{what}");
    }
    Ok(())
}

#[test]
fn a_gap_at_the_end_plays_only_what_is_queued_into_it() -> Result<(), AppError> {
    let rows: [StepRow; 4] = [
        ("the track's end, repeat off", QueueState::advance, (None, None)),
        (
            "the track's end under Repeat All",
            |q| {
                q.set_repeat_mode(RepeatMode::All);
                q.advance()
            },
            (Some(1), Some(0)),
        ),
        ("Previous", QueueState::previous, (Some(4), Some(3))),
        (
            "Play Next, then the track's end",
            |q| {
                q.insert_next(&[9]);
                q.advance()
            },
            (Some(9), Some(4)),
        ),
    ];
    for (what, step, expected) in rows {
        let mut q = gapped_at(4)?;

        let played = step(&mut q);

        assert_eq!((played, q.playing_index()), expected, "{what}");
    }
    Ok(())
}

#[test]
fn a_move_keeps_the_gap_between_the_rows_it_sat_between() -> Result<(), AppError> {
    let rows: [MoveRow; 4] = [
        ("a row dropped into the gap", 1, &[3], 1, Some(5)),
        ("the row after the gap dragged away", 1, &[1], 3, Some(4)),
        ("a played row moved past the gap", 1, &[0], 3, Some(3)),
        ("a row moved ahead of a gap at the end", 4, &[3], 0, None),
    ];
    for (what, gap, moved, to, expected) in rows {
        let mut q = gapped_at(gap)?;

        q.move_rows(moved, to);

        assert_eq!(q.peek_next(), expected, "{what}");
    }
    Ok(())
}

/// Next is lit off `has_next`, so the two disagreeing is a button that does nothing or a step the
/// button refuses.
#[test]
fn next_is_offered_exactly_where_next_has_a_row_to_go_to() -> Result<(), AppError> {
    let rows: [(&str, TryBuilt, RepeatMode, bool); 6] = [
        ("a gap mid-queue, repeat off", || gapped_at(1), RepeatMode::Off, true),
        ("a gap at the end, repeat off", || gapped_at(4), RepeatMode::Off, false),
        ("a gap at the end, Repeat All", || gapped_at(4), RepeatMode::All, true),
        ("a gap at the end, Repeat One", || gapped_at(4), RepeatMode::One, true),
        ("nothing current, repeat off", || Ok(queue_of(&FIVE)), RepeatMode::Off, true),
        ("an empty queue, Repeat All", || Ok(QueueState::default()), RepeatMode::All, false),
    ];
    for (what, build, mode, expected) in rows {
        let mut q = build()?;
        q.set_repeat_mode(mode);

        let answers = (q.has_next(), q.clone().advance_skip().is_some());

        assert_eq!(answers, (expected, expected), "{what}");
    }
    Ok(())
}

// ── clear and restore_cleared ───────────────────────────────────────

#[test]
fn an_undone_clear_comes_back_ahead_of_what_was_queued_since() {
    let mut q = queue_of(&[1, 2, 3]);
    q.seat(1);
    let held = q.take_cleared();
    q.add_tracks(&[7]);

    q.restore_cleared(held.into_iter().collect());

    assert_eq!(shown(&q), (vec![1, 2, 3, 7], Some(1)));
}

#[test]
fn a_run_of_clears_undone_together_comes_back_oldest_first() {
    let mut q = queue_of(&[1, 2]);
    let first = q.take_cleared();
    q.add_tracks(&[5, 6]);
    let second = q.take_cleared();

    q.restore_cleared(first.into_iter().chain(second).collect());

    assert_eq!(q.play_ids().to_vec(), [1, 2, 5, 6]);
}

#[test]
fn an_undone_clear_over_a_gap_at_the_end_plays_what_was_queued_since() -> Result<(), AppError> {
    let mut q = queue_of(&[1, 2, 3]);
    q.seat(2);
    cut(&mut q, &[2])?;
    let held = q.take_cleared();
    q.add_tracks(&[7]);

    q.restore_cleared(held.into_iter().collect());

    assert_eq!((q.playing_index(), q.peek_next()), (None, Some(7)));
    Ok(())
}

// ── replace and restore_replaced ────────────────────────────────────

#[test]
fn a_play_holds_the_queue_it_replaces_only_where_it_was_built_by_hand() {
    let rows: [(&str, Built, bool); 10] = [
        ("an empty queue", QueueState::default, false),
        (
            "a queue a play built",
            || {
                let mut q = QueueState::default();
                q.replace(&[7, 8], 0);
                q
            },
            false,
        ),
        ("a queue built by hand", || queue_of(&[7, 8]), true),
        ("a hand-built queue of the play's own ids", || queue_of(&PLAYED), false),
        (
            "the play's own ids, shuffled",
            || {
                let mut q = queue_of(&PLAYED);
                q.apply_shuffle_order(&[2, 0, 1]);
                q
            },
            false,
        ),
        (
            "a play's queue reordered by hand",
            || {
                let mut q = QueueState::default();
                q.replace(&PLAYED, 0);
                q.move_rows(&[0], 2);
                q
            },
            true,
        ),
        (
            "a play's queue with a row queued onto it",
            || {
                let mut q = QueueState::default();
                q.replace(&[7, 8], 0);
                q.add_tracks(&[9]);
                q
            },
            true,
        ),
        ("a queue restored at launch", || QueueState::restored(vec![7, 8], Some(0)), true),
        (
            "a hand-built queue cleared and put back",
            || {
                let mut q = queue_of(&[7, 8]);
                let held = q.take_cleared().into_iter().collect();
                q.restore_cleared(held);
                q
            },
            true,
        ),
        (
            "a hand-built queue a play replaced and put back",
            || {
                let mut q = queue_of(&[7, 8]);
                if let Some(held) = q.replace(&PLAYED, 0) {
                    q.restore_replaced(held, &mut rand::rng());
                }
                q
            },
            true,
        ),
    ];
    for (what, build, held) in rows {
        assert_eq!(build().replace(&PLAYED, 0).is_some(), held, "{what}");
    }
}

#[test]
fn an_undone_play_puts_back_the_queue_and_the_row_that_was_playing() -> Result<(), AppError> {
    let mut q = queue_of(&[7, 8, 9]);
    q.seat(1);
    let held = q
        .replace(&PLAYED, 0)
        .ok_or_else(|| AppError::validation_msg("a hand-built queue is held"))?;

    q.restore_replaced(held, &mut rand::rng());

    assert_eq!(shown(&q), (vec![7, 8, 9], Some(1)));
    Ok(())
}

#[test]
fn an_undone_play_puts_back_the_gap_a_removed_playing_row_left() -> Result<(), AppError> {
    let rows = [("a gap mid-queue", 1, Some(9)), ("a gap at the end", 2, None)];
    for (what, gap, expected_next) in rows {
        let mut q = queue_of(&[7, 8, 9]);
        q.seat(gap);
        cut(&mut q, &[gap])?;
        let held =
            q.replace(&PLAYED, 0).ok_or_else(|| AppError::validation_msg("a cut queue is held"))?;

        q.restore_replaced(held, &mut rand::rng());

        assert_eq!((q.playing_index(), q.peek_next()), (None, expected_next), "{what}");
    }
    Ok(())
}

#[test]
fn an_undone_play_comes_back_in_insertion_order_once_shuffle_went_off() -> Result<(), AppError> {
    let mut q = queue_of(&[7, 8, 9]);
    q.apply_shuffle_order(&[2, 0, 1]);
    let held = q
        .replace(&PLAYED, 0)
        .ok_or_else(|| AppError::validation_msg("a hand-built queue is held"))?;
    q.unshuffle();

    q.restore_replaced(held, &mut rand::rng());

    assert_eq!(q.play_ids().to_vec(), [7, 8, 9]);
    Ok(())
}
