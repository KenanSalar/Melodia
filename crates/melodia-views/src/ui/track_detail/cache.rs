//! The Rust half of an open detail's track list: which entity it lists, under what sort and
//! filter, and the tickets ordering its loads. The rows are the model's, which keeps the ids and
//! only the rows near the viewport, so a click, a sort or a keystroke reads none of them back.

use parking_lot::Mutex;
use slint::{StrongHandle, Weak};

use crate::ui::paged_rows::{KeysetTickets, ScrollPoint};
use melodia_core::entities::track_list::{SortDir, TrackOrder};
use melodia_core::utils::needle::{Needle, fold_needle};

pub struct DetailCache<G: StrongHandle> {
    /// The detail global, for a load landing from off the event loop.
    global: Weak<G>,
    /// The entity on screen, `-1` while the grid is. The library-changed subscriber and the grid
    /// prewarm both ask it.
    id: Mutex<i64>,
    /// The sort and the filter every load runs under. Written where the user picks them and read
    /// once a load holds its ticket, so the newest load runs under what the page shows.
    order: Mutex<TrackOrder>,
    filter: Mutex<Needle>,
    pub(super) tickets: KeysetTickets,
    /// Where the list was last read, which every load seeds its rows around.
    pub(super) scroll: ScrollPoint,
}

impl<G: StrongHandle> DetailCache<G> {
    pub fn new(global: Weak<G>, scroll: ScrollPoint) -> Self {
        Self {
            global,
            id: Mutex::new(-1),
            order: Mutex::new(TrackOrder::new("", SortDir::Asc)),
            filter: Mutex::new(Needle::default()),
            tickets: KeysetTickets::default(),
            scroll,
        }
    }

    pub fn global(&self) -> &Weak<G> {
        &self.global
    }

    pub fn id(&self) -> i64 {
        *self.id.lock()
    }

    pub fn set_id(&self, id: i64) {
        *self.id.lock() = id;
    }

    pub fn set_order(&self, order: TrackOrder) {
        *self.order.lock() = order;
    }

    pub fn set_filter(&self, text: &str) {
        *self.filter.lock() = fold_needle(text);
    }

    pub fn needle(&self) -> Needle {
        self.filter.lock().clone()
    }

    pub fn is_filtered(&self) -> bool {
        !self.filter.lock().is_empty()
    }

    pub fn clear(&self) {
        self.set_id(-1);
        self.filter.lock().clear();
    }

    /// The entity, sort and filter a load runs under.
    pub(super) fn query(&self) -> (i64, TrackOrder, Needle) {
        (self.id(), *self.order.lock(), self.needle())
    }

    /// Seats a fresh open of `id` under `order`, unfiltered.
    pub(super) fn seat(&self, id: i64, order: TrackOrder) {
        self.set_id(id);
        self.set_order(order);
        self.filter.lock().clear();
    }
}
