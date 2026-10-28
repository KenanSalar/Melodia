//! The chunking a built card grid does before it can hand Slint a model. A grid's `ListView`
//! virtualizes by *row*, so a flat card list becomes `ceil(n / columns)` row structs each holding
//! its cards as a nested model. The library grids page theirs through `ui::paged_rows::grid`;
//! Browse and Radio chunk here.

use std::rc::Rc;

use slint::{ModelRc, VecModel};

/// Chunk `cards` into rows of `columns`, **moving** each card into its row's model and
/// wrapping each row through `row`. `columns` is floored at one — a grid mid-layout can
/// report zero, and a zero-width chunk is a panic rather than an empty grid.
pub fn chunk_built_rows<C, R>(cards: Vec<C>, columns: i32, row: impl Fn(ModelRc<C>) -> R) -> Vec<R>
where
    C: Clone + 'static,
{
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut rows: Vec<R> = Vec::with_capacity(cards.len().div_ceil(cols));
    let mut cards = cards.into_iter();
    loop {
        // `Take` over a `vec::IntoIter` reports an exact `size_hint`, so each chunk
        // allocates once at its final size.
        let chunk: Vec<C> = cards.by_ref().take(cols).collect();
        if chunk.is_empty() {
            return rows;
        }
        rows.push(row(ModelRc::from(Rc::new(VecModel::from(chunk)))));
    }
}

#[cfg(test)]
#[path = "tests/grid_rows_tests.rs"]
mod tests;
