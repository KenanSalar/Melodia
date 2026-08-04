//! The chunk every card grid does before it can hand Slint a model.
//!
//! Each grid's `ListView` virtualizes by *row*, so the chunking is the
//! virtualization boundary: a flat card list becomes `ceil(n / columns)`
//! one-field grid-row structs, each holding that row's cards as a nested
//! model. Five surfaces do it — the four entity grids and both Favorites
//! grid tabs through one call — and they differ only in the card type, the
//! row type, and how a card is built.
//!
//! Kept out of [`crate::ui::grid_prewarm`] on purpose: that module is the
//! grids' shared *cover* machinery, and nothing here touches a cache.

use std::rc::Rc;

use slint::{ModelRc, VecModel};

/// Chunk `items` into rows of `columns`, mapping each item through `card` and
/// wrapping each row's cards through `row`.
///
/// `columns` is floored at one — a grid mid-layout can report zero, and a
/// zero-width chunk is a panic rather than an empty grid.
pub fn chunk_rows<T, C, R>(
    items: &[T],
    columns: i32,
    card: impl Fn(&T) -> C,
    row: impl Fn(ModelRc<C>) -> R,
) -> Vec<R>
where
    C: Clone + 'static,
{
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut rows: Vec<R> = Vec::with_capacity(items.len().div_ceil(cols));
    for chunk in items.chunks(cols) {
        let cards: Vec<C> = chunk.iter().map(&card).collect();
        rows.push(row(ModelRc::from(Rc::new(VecModel::from(cards)))));
    }
    rows
}

#[cfg(test)]
#[path = "tests/grid_rows_tests.rs"]
mod tests;
