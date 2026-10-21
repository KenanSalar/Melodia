//! The sheet's picks over a queue holding a track twice. Picks are by track, as on every other
//! list, so both of its rows are picked and both are what Remove takes.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use slint::Model;

use super::picked_positions;
use crate::ui::paged_rows::queue_rows::QueueRows;
use crate::ui::paged_rows::{Keyset, ReadStamp};

/// The sheet over a queue playing 1, 2, 1, 3, no row fetched.
fn sheet_over_a_repeat() -> Rc<QueueRows> {
    let rows = QueueRows::new(|_request| {});
    rows.show(Keyset { ids: Arc::from([1, 2, 1, 3]), seed: Vec::new(), read: ReadStamp::now() });
    rows
}

/// Picking either row of a track queued twice picks both, which is what the batch entries then
/// act on and what the count beside them says.
#[test]
fn picking_a_track_queued_twice_marks_both_of_its_rows() {
    let rows = sheet_over_a_repeat();

    rows.select(&HashSet::from([1]));

    let marked: Vec<bool> = rows.iter().map(|row| row.selected).collect();
    assert_eq!(marked, [true, false, true, false]);
}

/// Remove takes every row a pick names, so it can't leave one copy of a track queued twice behind,
/// still marked as picked.
#[test]
fn removing_the_picks_takes_both_copies_of_a_track_queued_twice() {
    let rows = sheet_over_a_repeat();

    let positions = picked_positions(&rows, &HashSet::from([1]));

    assert_eq!(positions, [0, 2]);
}
