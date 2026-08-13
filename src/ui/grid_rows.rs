//! The chunk every card grid does before it can hand Slint a model.
//!
//! Each grid's `ListView` virtualizes by *row*, so the chunking is the
//! virtualization boundary: a flat card list becomes `ceil(n / columns)`
//! one-field grid-row structs, each holding that row's cards as a nested
//! model. Every card grid does it, and they differ only in the card type, the
//! row type, and how a card is built.
//!
//! **That last difference is what splits the two entry points.** A caller that
//! keeps its source and projects a card out of it borrows ([`chunk_rows`] — the
//! four entity grids, which chunk indices against a `GridData`); a caller that
//! builds its cards in the same walk that filters them hands the `Vec` over
//! ([`chunk_built_rows`] — the three grid tabs and Browse). Reaching for the
//! borrowing form with `Clone::clone` as the projection is how the second group
//! ends up cloning every card it was about to throw away.
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

/// [`chunk_rows`] for a caller that already owns its cards — it **moves** them
/// into the per-row models instead of projecting new ones out of a source.
///
/// The two shapes are not interchangeable and the split is the whole point. The
/// four entity grids chunk *indices* and build each card from a `GridData` they
/// keep, so they need the borrowing form and its `card` closure. The tabbed
/// pages and Browse build a flat `Vec` in the same walk that filters it and then
/// drop it — handing that to `chunk_rows` meant `Clone::clone` as the
/// projection, i.e. a second full pass cloning every card into the chunk it was
/// about to be replaced by. Three `SharedString` refcount bumps and a struct
/// copy per card is small; doing it per card of an uncapped library-wide grid,
/// on the UI thread, once per finished track, is not.
///
/// Cannot move off the UI thread, which is why dropping the clone is the whole
/// available win: the per-row model is an `Rc<VecModel<_>>` and `Rc` is `!Send`.
pub fn chunk_built_rows<C, R>(cards: Vec<C>, columns: i32, row: impl Fn(ModelRc<C>) -> R) -> Vec<R>
where
    C: Clone + 'static,
{
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut rows: Vec<R> = Vec::with_capacity(cards.len().div_ceil(cols));
    let mut cards = cards.into_iter();
    loop {
        // `Take` over a `vec::IntoIter` reports an exact `size_hint`, so each
        // chunk allocates once at its final size.
        let chunk: Vec<C> = cards.by_ref().take(cols).collect();
        if chunk.is_empty() {
            return rows;
        }
        rows.push(row(ModelRc::from(Rc::new(VecModel::from(chunk)))));
    }
}

/// Chunk a flat list of already-built cards into `EntityCardGrid` rows.
///
/// The `EntityStripRow` → `EntityGridRow` specialisation of
/// [`chunk_built_rows`], for the tabbed pages: their cards are built by the walk
/// that filters them, where the four entity grids chunk *indices* and project
/// out of a `GridData`.
pub fn chunk_entity_rows(rows: Vec<UiEntityStripRow>, columns: i32) -> Vec<UiEntityGridRow> {
    chunk_built_rows(rows, columns, |entities| UiEntityGridRow { entities })
}

/// Swap a grid's rows in, or log and leave the model alone if the downcast
/// fails. `label` names the model in that log line — the two grid tabs, the one
/// Recently Played has and Browse's card grid share this, and a bare "downcast
/// failed" wouldn't say which.
///
/// Generic over the row type because Browse's grid holds `BrowseCardGridRow`
/// rather than `EntityGridRow`, and had grown a byte-for-byte copy of this
/// saying so in a comment. The `'static` bound is what `downcast_ref` needs; the
/// row structs Slint generates are all plain data and satisfy it.
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
