//! The panel's picks over a queue holding a track twice. A pick names one queued copy, so the other
//! copy of its track stays unpicked and out of what Remove takes.

use std::rc::Rc;

use slint::Model;

use crate::ui::paged_rows::queue_rows::QueueRows;
use melodia_engine::player::engine::queue::QueueState;

/// The panel over a queue playing 1, 2, 1, 3, no row fetched.
fn panel_over_a_repeat() -> Rc<QueueRows> {
    let mut queue = QueueState::default();
    queue.add_tracks(&[1, 2, 1, 3]);
    let rows = QueueRows::new(|_request| {});
    rows.show(queue.play_ids(), queue.play_entries());
    rows
}

/// A click on one row of a track queued twice marks that row and leaves the other copy alone,
/// which is also what a drag of the picks then carries.
#[test]
fn picking_one_copy_of_a_track_queued_twice_marks_that_row_alone() {
    let rows = panel_over_a_repeat();

    rows.click(0, true, false);

    let marked: Vec<bool> = rows.iter().map(|row| row.selected).collect();
    assert_eq!(marked, [true, false, false, false]);
}

/// Remove takes the picked rows alone, so it can't delete the copy of a picked track the user left
/// unpicked.
#[test]
fn removing_the_picks_leaves_the_unpicked_copy_of_their_track() {
    let rows = panel_over_a_repeat();

    rows.click(2, true, false);

    assert_eq!(rows.picked_rows(), [2]);
}
