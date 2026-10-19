//! A card grid over a [`PagedRows`]: the grid's ordered ids and its cached cards are the store's,
//! and Slint is handed `ceil(n / columns)` chunk rows over them.
//!
//! **A chunk reads its cards from the store as Slint asks**, so no card is copied into a row
//! model, and a column change is index arithmetic that keeps every cached card.
//!
//! **The same chunk keeps the same model while Slint holds it.** A grid's row repeater compares
//! models by pointer and rebuilds every card of a row whose model is new (`slint-pitfalls.md`, the
//! entry on a `for` over a callback's model), so a chunk is built once per generation and handed
//! back on every read until a new keyset or column count starts the next. That is also what lets a
//! card that lands repaint its one cell: the store reports the position, and the chunk holding it
//! repaints that card. A chunk no row instance holds any more is dropped on the next build, which
//! bounds them by what is on screen.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use slint::{Model, ModelNotify, ModelRc, ModelTracker};

use super::{Keyset, PagedRow, PagedRows, RowNotify, RowRequest, ScrollPoint};
use crate::ui::util::clamp_i64_to_i32;

/// One grid row: up to a row's worth of cards in a nested model, the shape every `*GridRow`
/// struct in `models.slint` has.
pub trait GridRow: Clone + 'static {
    type Card: PagedRow;

    fn of(cards: ModelRc<Self::Card>) -> Self;
}

type Store<G> = PagedRows<<G as GridRow>::Card, Chunks<G>>;

/// Where a grid's store reports its changes: the chunk models Slint holds, and the grid's own rows.
pub struct Chunks<G: GridRow> {
    columns: Cell<usize>,
    built: RefCell<HashMap<usize, Rc<Chunk<G>>>>,
    notify: ModelNotify,
}

impl<G: GridRow> RowNotify for Chunks<G> {
    fn reset(&self) {
        self.built.borrow_mut().clear();
        self.notify.reset();
    }

    fn row_changed(&self, row: usize) {
        let columns = self.columns.get();
        let chunk = self.built.borrow().get(&(row / columns)).cloned();
        if let Some(chunk) = chunk {
            chunk.notify.row_changed(row % columns);
        }
    }

    // Every card after the one that moved changes chunk.
    fn row_added(&self, _index: usize, _count: usize) {
        self.reset();
    }

    fn row_removed(&self, _index: usize, _count: usize) {
        self.reset();
    }
}

/// One grid row's cards, read through to the store.
struct Chunk<G: GridRow> {
    store: Weak<Store<G>>,
    first: usize,
    columns: usize,
    notify: ModelNotify,
}

impl<G: GridRow> Model for Chunk<G> {
    type Data = G::Card;

    fn row_count(&self) -> usize {
        self.store
            .upgrade()
            .map_or(0, |store| store.len().saturating_sub(self.first).min(self.columns))
    }

    fn row_data(&self, column: usize) -> Option<G::Card> {
        if column >= self.columns {
            return None;
        }
        self.store.upgrade()?.read(self.first + column)
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

pub struct PagedGrid<G: GridRow> {
    store: Rc<Store<G>>,
}

impl<G: GridRow> PagedGrid<G> {
    /// An empty grid one column wide, whose cards come from `fetch`.
    pub fn new(fetch: impl Fn(RowRequest<G::Card>) + 'static) -> Rc<Self> {
        let chunks = Chunks {
            columns: Cell::new(1),
            built: RefCell::default(),
            notify: ModelNotify::default(),
        };
        Rc::new(Self { store: PagedRows::with_notify(fetch, chunks) })
    }

    /// The grid behind `rows`, where it is one.
    pub fn of(rows: &ModelRc<G>) -> Option<&Self> {
        rows.as_any().downcast_ref()
    }

    /// [`Self::of`] for a grid installed with one, logging a miss under `label`.
    pub fn installed<'a>(rows: &'a ModelRc<G>, label: &str) -> Option<&'a Self> {
        let found = Self::of(rows);
        if found.is_none() {
            log::warn!("{label}: PagedGrid downcast failed, cards left as they were");
        }
        found
    }

    /// Chunk the cards `columns` to a row, floored at one: a grid mid-layout can report zero. A
    /// new count rebuilds the rows Slint holds and keeps every cached card.
    pub fn set_columns(&self, columns: i32) {
        let columns = usize::try_from(columns.max(1)).unwrap_or(1);
        let chunks = self.store.notifier();
        if chunks.columns.replace(columns) != columns {
            chunks.reset();
        }
    }

    /// [`PagedRows::show`].
    pub fn show(&self, keyset: Keyset<G::Card>) {
        self.store.show(keyset);
    }

    /// [`PagedRows::reread`].
    pub fn reread(&self) {
        self.store.reread();
    }

    /// [`PagedRows::clear`].
    pub fn clear(&self) {
        self.store.clear();
    }

    /// Every card's id, in order.
    pub fn ids(&self) -> Arc<[i64]> {
        self.store.ids()
    }

    /// Every card's id, in order, as the card selection names them.
    pub fn card_ids(&self) -> Vec<i32> {
        self.store.ids().iter().map(|&id| clamp_i64_to_i32(id)).collect()
    }

    /// [`PagedRows::scroll_point`].
    pub fn scroll_point(&self) -> ScrollPoint {
        self.store.scroll_point()
    }

    /// [`PagedRows::cached`].
    pub fn cached(&self, id: i64) -> Option<G::Card> {
        self.store.cached(id)
    }
}

impl<G: GridRow> Model for PagedGrid<G> {
    type Data = G;

    fn row_count(&self) -> usize {
        self.store.len().div_ceil(self.store.notifier().columns.get())
    }

    fn row_data(&self, row: usize) -> Option<G> {
        if row >= self.row_count() {
            return None;
        }
        let chunks = self.store.notifier();
        let mut built = chunks.built.borrow_mut();
        if let Some(chunk) = built.get(&row) {
            return Some(G::of(ModelRc::from(Rc::clone(chunk))));
        }
        // Held only here, so no row instance shows it any more.
        built.retain(|_, chunk| Rc::strong_count(chunk) > 1);
        let columns = chunks.columns.get();
        let chunk = Rc::new(Chunk {
            store: Rc::downgrade(&self.store),
            first: row * columns,
            columns,
            notify: ModelNotify::default(),
        });
        built.insert(row, Rc::clone(&chunk));
        Some(G::of(ModelRc::from(chunk)))
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.store.notifier().notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
