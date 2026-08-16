//! The chunk every card grid does before it can hand Slint a model.
//!
//! Each grid's `ListView` virtualizes by *row*, so the chunking is the virtualization
//! boundary: a flat card list becomes `ceil(n / columns)` one-field row structs, each
//! holding that row's cards as a nested model. Every card grid does it, differing only in
//! the card type, the row type, and how a card is built.
//!
//! **That last difference is what splits the two entry points.** A caller that keeps its
//! source and projects a card out of it borrows ([`chunk_rows`]); one that builds its cards
//! in the same walk that filters them hands the `Vec` over ([`chunk_built_rows`]). Reaching
//! for the borrowing form with `Clone::clone` as the projection is how the second group
//! clones every card it was about to throw away.
//!
//! Kept out of [`crate::ui::grid_prewarm`]: that module is the grids' shared *cover*
//! machinery, and nothing here touches a cache.

use std::rc::Rc;

use slint::{Model, ModelRc, VecModel};

use crate::{EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow};

/// Chunk `items` into rows of `columns`, mapping each item through `card` and wrapping each
/// row's cards through `row`. `columns` is floored at one — a grid mid-layout can report
/// zero, and a zero-width chunk is a panic rather than an empty grid.
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

/// [`chunk_rows`] for a caller that already owns its cards — it **moves** them into the
/// per-row models instead of projecting new ones out of a source.
///
/// The four entity grids chunk *indices* and build each card from a `GridData` they keep,
/// so they need the borrowing form; the tabbed pages and Browse build a flat `Vec` in the
/// same walk that filters it and then drop it, and handing that to [`chunk_rows`] means
/// `Clone::clone` as the projection — a second full pass cloning every card into the chunk
/// about to replace it, on the UI thread, once per finished track. None of it can move off
/// that thread (`Rc<VecModel<_>>` is `!Send`), so dropping the clone is the whole win.
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

/// The `EntityStripRow` → `EntityGridRow` specialisation of [`chunk_built_rows`], for
/// the tabbed pages, whose cards are built by the walk that filters them.
pub fn chunk_entity_rows(rows: Vec<UiEntityStripRow>, columns: i32) -> Vec<UiEntityGridRow> {
    chunk_built_rows(rows, columns, |entities| UiEntityGridRow { entities })
}

/// Swap a grid's rows in, or log and leave the model alone if the downcast fails. `label`
/// names the model in that line, four grids sharing this. Generic over the row type
/// because Browse's grid holds `BrowseCardGridRow` rather than `EntityGridRow`.
pub fn write_grid<R: Clone + 'static>(model: &ModelRc<R>, rows: Vec<R>, label: &str) {
    let Some(vec) = model.as_any().downcast_ref::<VecModel<R>>() else {
        log::warn!("{label}: VecModel<{}> downcast failed", std::any::type_name::<R>());
        return;
    };
    vec.set_vec(rows);
}

#[cfg(test)]
#[path = "tests/grid_rows_tests.rs"]
mod tests;
