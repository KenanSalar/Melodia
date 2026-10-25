//! The queue panel's model: the row shape it and Up Next both draw, and the picks it keeps by queue
//! entry, over a real queue.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use slint::{FilterModel, Model, ModelRc};

use super::{QueueRows, to_queue_row};
use crate::ui::fixtures::track_row;
use crate::ui::util::StringPool;
use melodia_core::entities::track::TrackListRow;
use melodia_engine::player::engine::queue::QueueState;
use melodia_ui::QueueRow;

fn queue_row(row: &TrackListRow) -> QueueRow {
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

// --- picks over a track queued twice ---

/// What an edit does to the queue before the panel shows it.
type Edit = fn(&mut QueueState);

/// A queue of 1, 2, 1, 3, nothing playing: track 1 sits on rows 0 and 2.
fn repeat_queue() -> QueueState {
    let mut queue = QueueState::default();
    queue.add_tracks(&[1, 2, 1, 3]);
    queue
}

/// The panel showing `queue`, no row fetched.
fn panel_over(queue: &QueueState) -> Rc<QueueRows> {
    let rows = QueueRows::new(|_request| {});
    rows.show(queue.play_ids(), queue.play_entries());
    rows
}

/// Each row's flag, as the panel stamps it.
fn marked(rows: &QueueRows) -> Vec<bool> {
    rows.iter().map(|row| row.selected).collect()
}

/// A click on one row of a track queued twice marks that row and leaves the other copy alone,
/// which is also what a drag of the picks then carries.
#[test]
fn picking_one_copy_of_a_track_queued_twice_marks_that_row_alone() {
    let rows = panel_over(&repeat_queue());

    rows.click(0, true, false);

    assert_eq!(marked(&rows), [true, false, false, false]);
}

#[test]
fn a_ctrl_click_on_the_other_copy_marks_both() {
    let rows = panel_over(&repeat_queue());
    rows.click(0, false, false);

    rows.click(2, true, false);

    assert_eq!(marked(&rows), [true, false, true, false]);
}

/// The anchor is the row clicked rather than its track, so a range from the second copy of track 1
/// doesn't reach back to the first.
#[test]
fn a_shift_range_runs_from_the_copy_clicked() {
    let rows = panel_over(&repeat_queue());
    rows.click(2, false, false);

    rows.click(3, false, true);

    assert_eq!(marked(&rows), [false, false, true, true]);
}

/// Remove takes the picked rows alone, so it can't delete the copy of a picked track the user left
/// unpicked.
#[test]
fn removing_the_picks_leaves_the_unpicked_copy_of_their_track() {
    let rows = panel_over(&repeat_queue());

    rows.click(2, true, false);

    assert_eq!(rows.picked_rows(), [2]);
}

/// The row menu labels its batch entries with the count and hands them the tracks, so both copies
/// picked are two rows to remove and one track to add to a playlist.
#[test]
fn the_picks_count_rows_and_name_each_track_once_in_queue_order() {
    let rows = panel_over(&repeat_queue());
    rows.click(3, false, false);
    rows.click(2, true, false);

    rows.click(0, true, false);

    assert_eq!(rows.picked_tracks(), (3, vec![1, 3]));
}

/// A drop let go below the last row names no entry, which the queue takes as the end.
#[test]
fn no_entry_sits_past_the_last_row() {
    let rows = panel_over(&repeat_queue());

    assert_eq!(rows.entry_at(4), None);
}

/// The second copy of track 1 picked and followed through each edit that keeps its row, the panel
/// showing the queue after every edit as each publish does.
#[test]
fn a_pick_stays_on_its_row_through_every_edit_that_keeps_it() {
    let cases: [(&str, &[Edit], &[bool]); 8] = [
        (
            "an insert ahead of it",
            &[|q| {
                let first = q.entry_at(0);
                q.insert_before(first, &[9]);
            }],
            &[false, false, false, true, false],
        ),
        ("a Play Next", &[|q| q.insert_next(&[8, 9])], &[false, false, false, false, true, false]),
        (
            "a move of the picked row",
            &[|q| {
                q.move_rows(&[2], 0);
            }],
            &[true, false, false, false],
        ),
        (
            "a move of another row past it",
            &[|q| {
                q.move_rows(&[0], 3);
            }],
            &[false, true, false, false],
        ),
        ("a shuffle", &[|q| q.apply_shuffle_order(&[2, 3, 0, 1])], &[true, false, false, false]),
        (
            "a shuffle and an unshuffle",
            &[|q| q.apply_shuffle_order(&[2, 3, 0, 1]), QueueState::unshuffle],
            &[false, false, true, false],
        ),
        (
            "a removal of another row",
            &[|q| {
                q.remove_at(0);
            }],
            &[false, true, false],
        ),
        (
            "a prune of another track",
            &[|q| {
                q.prune_missing(&HashSet::from([2]));
            }],
            &[false, true, false],
        ),
    ];
    for (case, edits, expected) in cases {
        let mut queue = repeat_queue();
        let rows = panel_over(&queue);
        rows.click(2, true, false);

        for edit in edits {
            edit(&mut queue);
            rows.show(queue.play_ids(), queue.play_entries());
        }

        assert_eq!(marked(&rows), expected, "{case}");
    }
}

/// Picks name entries rather than tracks, so the copy of track 1 left behind doesn't inherit one.
#[test]
fn removing_the_picked_row_takes_its_pick_with_it() {
    let mut queue = repeat_queue();
    let rows = panel_over(&queue);
    rows.click(2, true, false);

    queue.remove_at(2);
    rows.show(queue.play_ids(), queue.play_entries());

    assert_eq!(marked(&rows), [false, false, false]);
}

type Reread = Box<dyn Fn(&QueueRow) -> bool>;

/// The panel with a `slint::FilterModel` standing in for the `ListView`, which reads a row again
/// exactly when the panel says it changed, so what it records is what a list would repaint.
struct Watched {
    rows: Rc<QueueRows>,
    /// Subscribed to the panel for as long as it lives, which is all it is held for.
    _list: FilterModel<ModelRc<QueueRow>, Reread>,
    reread: Rc<RefCell<Vec<(i32, bool)>>>,
}

impl Watched {
    /// The panel over `queue` with `picked` Ctrl-clicked, and nothing read back since.
    fn picking(queue: &QueueState, picked: &[i32]) -> Self {
        let rows = panel_over(queue);
        for &row in picked {
            rows.click(row, true, false);
        }
        let reread = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&reread);
        let record: Reread = Box::new(move |row| {
            recorder.borrow_mut().push((row.id, row.selected));
            true
        });
        let list = FilterModel::new(ModelRc::from(Rc::clone(&rows)), record);
        reread.borrow_mut().clear();
        Self { rows, _list: list, reread }
    }

    /// Each row read back, as its track and its flag.
    fn reread(&self) -> Vec<(i32, bool)> {
        self.reread.borrow().clone()
    }
}

/// A queue replaced by the same tracks keeps its ids, so the store repaints none of its rows, and
/// a `ListView` would go on drawing the old picks over copies nobody picked.
#[test]
fn a_queue_refilled_with_the_same_tracks_repaints_the_rows_it_unpicked() {
    let mut queue = repeat_queue();
    let watched = Watched::picking(&queue, &[1, 2]);

    queue.clear();
    queue.add_tracks(&[1, 2, 1, 3]);
    watched.rows.show(queue.play_ids(), queue.play_entries());

    assert_eq!(watched.reread(), [(2, false), (1, false)]);
}

/// Hiding the panel empties it, and the queue it comes back to is the one the picks were made in.
#[test]
fn a_pick_survives_a_hide() {
    let queue = repeat_queue();
    let rows = panel_over(&queue);
    rows.click(2, true, false);
    rows.clear();

    rows.show(queue.play_ids(), queue.play_entries());

    assert_eq!(marked(&rows), [false, false, true, false]);
}

/// The other side of a hide: a queue emptied meanwhile leaves no pick to keep. An entry is never
/// queued again, so no row could show a stale one, and the set itself is what's read.
#[test]
fn a_hidden_panel_shown_an_emptied_queue_keeps_no_pick() {
    let mut queue = repeat_queue();
    let rows = panel_over(&queue);
    rows.click(2, false, false);
    rows.clear();

    queue.clear();
    rows.show(queue.play_ids(), queue.play_entries());

    assert_eq!((rows.picks.borrow().len(), rows.anchor.get()), (0, None));
}

/// A drag moves the rows on show ahead of the engine, so the pick, and the entry a drop then names
/// the row by, go with the row.
#[test]
fn a_move_carries_each_rows_entry_and_pick_with_it() {
    let queue = repeat_queue();
    let rows = panel_over(&queue);
    rows.click(2, true, false);

    rows.move_rows(&[2], 0);

    let moved = (rows.entry_at(0), marked(&rows));
    assert_eq!(moved, (queue.entry_at(2), vec![true, false, false, false]));
}
