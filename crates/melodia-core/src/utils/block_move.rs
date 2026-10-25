//! Moving several items of a list as one block, which the queue, the rows a list shows and a
//! playlist each do to a list of their own. One copy is what keeps the three landing a block in
//! the same place.

/// Where a block lands once the items at `positions` have left: `slot`, the gap above the item
/// that sat there, less every lifted item above it.
#[must_use]
pub fn landing(positions: &[usize], slot: usize) -> usize {
    slot - positions.partition_point(|&position| position < slot)
}

/// Moves the items at `positions`, ascending and distinct, so they sit together from `to` on in
/// the order they had, `to` being counted once they have left.
///
/// Answers whether anything moved. Nothing does for an empty or unsorted `positions`, one past the
/// end, a block that would run past the end, or one already sitting at `to`.
pub fn move_block<T>(items: &mut Vec<T>, positions: &[usize], to: usize) -> bool {
    let Some(&last) = positions.last() else {
        return false;
    };
    let ascending = positions.array_windows().all(|&[earlier, later]| earlier < later);
    let in_place = positions.iter().enumerate().all(|(offset, &position)| position == to + offset);
    if !ascending || last >= items.len() || to + positions.len() > items.len() || in_place {
        return false;
    }

    let mut lifted = positions.iter().copied().peekable();
    let mut moved = Vec::with_capacity(positions.len());
    let mut kept = Vec::with_capacity(items.len() - positions.len());
    for (position, item) in std::mem::take(items).into_iter().enumerate() {
        if lifted.next_if_eq(&position).is_some() {
            moved.push(item);
        } else {
            kept.push(item);
        }
    }
    kept.splice(to..to, moved);
    *items = kept;
    true
}

#[cfg(test)]
#[path = "tests/block_move_tests.rs"]
mod tests;
