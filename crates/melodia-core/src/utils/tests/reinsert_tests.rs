//! The merge every undo of a removal lands its rows through: the queue's, a playlist's.

use super::reinsert;

const KEPT: [char; 3] = ['a', 'b', 'c'];

/// Items put back at their indices.
type Held = &'static [(usize, char)];

/// What a row covers, what it puts back into [`KEPT`], and the list the merge answers.
type Row = (&'static str, Held, &'static [char]);

#[test]
fn each_held_item_lands_at_its_index_as_one_by_one_inserts_would() {
    let rows: [Row; 6] = [
        ("at the front", &[(0, 'x')], &['x', 'a', 'b', 'c']),
        ("two adjacent", &[(1, 'x'), (2, 'y')], &['a', 'x', 'y', 'b', 'c']),
        ("two apart", &[(1, 'x'), (3, 'y')], &['a', 'x', 'b', 'y', 'c']),
        ("either side of the first kept", &[(0, 'x'), (2, 'y')], &['x', 'a', 'y', 'b', 'c']),
        ("on the end", &[(3, 'x')], &['a', 'b', 'c', 'x']),
        ("all ahead", &[(0, 'x'), (1, 'y'), (2, 'z')], &['x', 'y', 'z', 'a', 'b', 'c']),
    ];
    for (what, held, expected) in rows {
        assert_eq!(reinsert(KEPT.to_vec(), held.iter().copied()), expected, "{what}");
    }
}

/// A restore lands its rows in a list that may have shrunk since the removal, so an index can
/// outrun it: those append, in their own order, rather than being lost.
#[test]
fn an_index_past_the_end_appends() {
    let rows: [Row; 3] = [
        ("one past", &[(4, 'x')], &['a', 'b', 'c', 'x']),
        ("far past", &[(99, 'x')], &['a', 'b', 'c', 'x']),
        ("two past, in order", &[(7, 'x'), (9, 'y')], &['a', 'b', 'c', 'x', 'y']),
    ];
    for (what, held, expected) in rows {
        assert_eq!(reinsert(KEPT.to_vec(), held.iter().copied()), expected, "{what}");
    }
}

#[test]
fn an_empty_side_leaves_the_other_as_it_was() {
    let rows: [(&str, &[char], Held, &[char]); 3] = [
        ("nothing held", &KEPT, &[], &['a', 'b', 'c']),
        ("nothing kept", &[], &[(0, 'x'), (5, 'y')], &['x', 'y']),
        ("neither", &[], &[], &[]),
    ];
    for (what, kept, held, expected) in rows {
        assert_eq!(reinsert(kept.to_vec(), held.iter().copied()), expected, "{what}");
    }
}
