//! Putting removed items back at the indices they left, for an undo.

/// Merges `held` into `kept`, each at its index, as inserting them one by one in ascending order
/// would: an earlier insert never shifts a later target, and an index past the end appends.
///
/// One walk rather than an insert per item, which is quadratic on a library-sized list.
#[must_use]
pub fn reinsert<T>(kept: Vec<T>, held: impl IntoIterator<Item = (usize, T)>) -> Vec<T> {
    let mut held = held.into_iter().peekable();
    let mut kept = kept.into_iter();
    let mut merged = Vec::with_capacity(kept.len() + held.size_hint().0);
    loop {
        if let Some((_, item)) = held.next_if(|&(index, _)| index <= merged.len()) {
            merged.push(item);
            continue;
        }
        let Some(item) = kept.next() else { break };
        merged.push(item);
    }
    merged.extend(held.map(|(_, item)| item));
    merged
}
