//! The one move rule the queue, the rows a list shows and a playlist share.

use super::{landing, move_block};

const ITEMS: [char; 6] = ['a', 'b', 'c', 'd', 'e', 'f'];

/// `ITEMS` once `positions` have moved to `to`, with whether anything did.
fn moved(positions: &[usize], to: usize) -> (bool, Vec<char>) {
    let mut items = ITEMS.to_vec();
    let answer = move_block(&mut items, positions, to);
    (answer, items)
}

#[test]
fn a_block_lands_together_at_to_in_the_order_it_had() {
    let rows: [(&str, &[usize], usize, [char; 6]); 7] = [
        ("one row down", &[1], 3, ['a', 'c', 'd', 'b', 'e', 'f']),
        ("one row up", &[4], 1, ['a', 'e', 'b', 'c', 'd', 'f']),
        ("a run down", &[1, 2], 3, ['a', 'd', 'e', 'b', 'c', 'f']),
        ("a run up", &[3, 4], 0, ['d', 'e', 'a', 'b', 'c', 'f']),
        ("scattered rows gathered", &[0, 2, 5], 1, ['b', 'a', 'c', 'f', 'd', 'e']),
        ("a run landing at the top", &[2, 3], 0, ['c', 'd', 'a', 'b', 'e', 'f']),
        ("a run landing at the end", &[0, 1], 4, ['c', 'd', 'e', 'f', 'a', 'b']),
    ];
    for (what, positions, to, expected) in rows {
        assert_eq!(moved(positions, to), (true, expected.to_vec()), "{what}");
    }
}

/// Each caller hands over what a selection or a drag produced, so a malformed block has to leave
/// the list as it was rather than panic in the splice or drop a row.
#[test]
fn a_block_that_cannot_move_leaves_the_items_untouched() {
    let rows: [(&str, &[usize], usize); 6] = [
        ("no positions", &[], 0),
        ("positions out of order", &[2, 1], 0),
        ("a position twice", &[1, 1], 0),
        ("a position one past the end", &[6], 0),
        ("a block running one past the end", &[0, 1], 5),
        ("a block already sitting at to", &[2, 3], 2),
    ];
    for (what, positions, to) in rows {
        assert_eq!(moved(positions, to), (false, ITEMS.to_vec()), "{what}");
    }
}

#[test]
fn a_slot_lands_a_block_less_every_lifted_item_above_it() {
    let rows: [(&str, &[usize], usize, usize); 9] = [
        ("before a run", &[2, 3, 4], 1, 1),
        ("after a run", &[2, 3, 4], 6, 3),
        ("at the end, past a run", &[2, 3, 4], 7, 4),
        ("above the first of scattered picks", &[1, 4], 0, 0),
        ("between scattered picks", &[1, 4], 3, 2),
        ("directly above the second pick", &[1, 4], 4, 3),
        ("directly below the second pick", &[1, 4], 5, 3),
        ("after scattered picks", &[1, 4], 6, 4),
        ("with nothing lifted", &[], 3, 3),
    ];
    for (what, positions, slot, expected) in rows {
        assert_eq!(landing(positions, slot), expected, "{what}");
    }
}

/// `ListDropTarget` refuses a drop on a run's own slots, a rule written only in that `.slint` file.
/// Here it holds on the Rust side too, so a slot that got past it still moves nothing.
#[test]
fn a_slot_inside_or_beside_a_run_lands_the_run_where_it_is() {
    let rows: [(&[usize], usize); 10] = [
        (&[2, 3, 4], 2),
        (&[2, 3, 4], 3),
        (&[2, 3, 4], 4),
        (&[2, 3, 4], 5),
        (&[0, 1], 0),
        (&[0, 1], 1),
        (&[0, 1], 2),
        (&[4, 5], 4),
        (&[4, 5], 5),
        (&[4, 5], 6),
    ];
    for (run, slot) in rows {
        let to = landing(run, slot);

        assert_eq!(moved(run, to), (false, ITEMS.to_vec()), "run {run:?} at slot {slot}");
    }
}
