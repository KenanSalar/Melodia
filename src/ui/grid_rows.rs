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

use slint::{Model, ModelRc, VecModel};

use crate::{EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow};

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

/// Chunk a flat list of already-built cards into `EntityCardGrid` rows.
///
/// The `EntityStripRow` → `EntityGridRow` specialisation of [`chunk_rows`], for
/// the tabbed pages: their cards are built by the walk that filters them, where
/// the four entity grids chunk *indices* and project out of a `GridData`.
pub fn chunk_entity_rows(rows: &[UiEntityStripRow], columns: i32) -> Vec<UiEntityGridRow> {
    chunk_rows(rows, columns, Clone::clone, |entities| UiEntityGridRow {
        entities,
    })
}

/// Swap a grid's rows in, or log and leave the model alone if the downcast
/// fails. `label` names the model in that log line — the two grid tabs and the
/// one Recently Played has share this, and a bare "downcast failed" wouldn't say
/// which.
pub fn write_grid(model: &ModelRc<UiEntityGridRow>, rows: Vec<UiEntityGridRow>, label: &str) {
    let Some(vec) = model.as_any().downcast_ref::<VecModel<UiEntityGridRow>>() else {
        log::warn!("{label}: VecModel<EntityGridRow> downcast failed");
        return;
    };
    vec.set_vec(rows);
}

#[cfg(test)]
#[path = "tests/grid_rows_tests.rs"]
mod tests;
