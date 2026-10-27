//! The queue panel's model, and the row shape Up Next draws too.
//!
//! The queue holds ids, as long as the library at its longest, so the panel fetches its rows by id
//! like any other list that can hold the whole library, through a [`PagedRows`] store. They come
//! through the track-list projection, which carries the album, artist and genre ids the row menu's
//! "Go to" entries need.
//!
//! **A pick names a queue entry, not a track.** The queue can hold a track twice, and each copy is
//! a row of its own to pick, drag and remove, so [`QueueRows`] keeps the engine's entries beside
//! the store's ids and stamps `selected` from them. The store still caches by track id, so the two
//! copies share one fetched row.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use slint::{Model, ModelRc, ModelTracker};

use super::track_rows::fetch_by_id;
use super::{Keyset, PagedRow, PagedRows, ReadStamp, RowRequest, window};
use crate::ui::list_selection::click_selection;
use crate::ui::tracks::format_duration_ms;
use crate::ui::util::{self, StringPool, clamp_i64_to_i32};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::error::AppResult;
use melodia_core::utils::block_move;
use melodia_engine::player::engine::queue::QueueEntry;
use melodia_ui::QueueRow;

/// The queue panel's rows: the queue's ids and entries in play order, the rows fetched for them,
/// and which entries are picked.
///
/// `entries` is as long as the store's keyset, row for row, and every method moving one moves the
/// other.
pub struct QueueRows {
    store: Rc<PagedRows<QueueRow>>,
    entries: RefCell<Arc<[QueueEntry]>>,
    picks: RefCell<HashSet<QueueEntry>>,
    /// The entry a Shift-click ranges from.
    anchor: Cell<Option<QueueEntry>>,
}

impl QueueRows {
    /// An empty panel whose rows come from `fetch`.
    pub fn new(fetch: impl Fn(RowRequest<QueueRow>) + 'static) -> Rc<Self> {
        Rc::new(Self {
            store: PagedRows::new(fetch),
            entries: RefCell::new(Arc::from([])),
            picks: RefCell::default(),
            anchor: Cell::new(None),
        })
    }

    /// The model behind `rows`, logging a miss under `label` rather than passing over it.
    pub fn installed<'a>(rows: &'a ModelRc<QueueRow>, label: &str) -> Option<&'a Self> {
        util::installed(rows, label)
    }

    /// Whether `ids` is the order on show: the same allocation rather than an equal one. The
    /// engine builds one per order, so a publish that moved only the current row hands it back.
    pub fn shows(&self, ids: &Arc<[i64]>) -> bool {
        Arc::ptr_eq(&self.store.ids(), ids)
    }

    /// Show the queue's `ids` and their `entries`, answering whether the entries moved. Where
    /// they did, the picks and the anchor keep only entries still queued and the rows whose flag
    /// changed repaint, a queue replaced by the same tracks keeping its ids and so its rows. A
    /// cleared panel's first show counts as a move, or picks kept across the clear would outlive
    /// a queue emptied meanwhile.
    pub fn show(&self, ids: Arc<[i64]>, entries: Arc<[QueueEntry]>) -> bool {
        let was_entries = self.entries.replace(entries);
        let moved = was_entries.is_empty() || *was_entries != **self.entries.borrow();
        self.store.show(Keyset { ids, seed: Vec::new(), read: ReadStamp::now() });
        if moved {
            self.keep_queued_picks(&was_entries);
        }
        moved
    }

    /// Store a seed fetched for the queue on show. A queue that moved since is already on show
    /// and fetching its own rows, so a seed for another order is dropped.
    pub fn land_seed(&self, keyset: Keyset<QueueRow>) {
        if *keyset.ids == *self.store.ids() {
            self.store.show(keyset);
        }
    }

    /// Read every row the panel holds again, those on screen first, after a library change that
    /// may have rewritten any of them. The ids are the queue's rather than a query's, so the stamp
    /// they are read again against is now.
    pub fn reread(&self) {
        self.store.show(Keyset { ids: self.store.ids(), seed: Vec::new(), read: ReadStamp::now() });
        self.store.reread();
    }

    /// Move the rows at `rows` together to `to`, as the store does, their entries and so their
    /// picks with them. Answers whether anything moved.
    pub fn move_rows(&self, rows: &[usize], to: usize) -> bool {
        let mut entries = self.entries.borrow().to_vec();
        if !block_move::move_block(&mut entries, rows, to) {
            return false;
        }
        *self.entries.borrow_mut() = Arc::from(entries);
        self.store.move_rows(rows, to)
    }

    /// Empty the list, its cache and its entries. The picks and the anchor stay for the next show,
    /// which keeps those still queued.
    pub fn clear(&self) {
        *self.entries.borrow_mut() = Arc::from([]);
        self.store.clear();
    }

    /// The entry on show at `row`, `None` past the last.
    pub fn entry_at(&self, row: usize) -> Option<QueueEntry> {
        self.entries.borrow().get(row).copied()
    }

    /// Edit the cached row of track `id`, which every copy of it reads.
    pub fn patch(&self, id: i64, edit: impl FnOnce(&mut QueueRow)) {
        self.store.patch(id, edit);
    }

    /// Pick by a click on row `idx`, by the rule every list clicks by.
    pub fn click(&self, idx: i32, ctrl: bool, shift: bool) {
        let entries = Arc::clone(&self.entries.borrow());
        let Some(&entry) = usize::try_from(idx).ok().and_then(|row| entries.get(row)) else {
            return;
        };
        let picked = self.picks.borrow().iter().copied().collect();
        let (picks, anchor) = click_selection(
            self.anchor.get(),
            picked,
            || entries.to_vec(),
            idx,
            entry,
            shift,
            ctrl,
        );
        self.select(picks.into_iter().collect(), Some(anchor));
    }

    /// Pick every row, the anchor staying put (`list_selection::select_all_rows`).
    pub fn select_all(&self) {
        let every = self.entries.borrow().iter().copied().collect();
        self.select(every, self.anchor.get());
    }

    /// Drop every pick, and the anchor with them.
    pub fn clear_picks(&self) {
        self.select(HashSet::new(), None);
    }

    /// Where the picked rows sit, in order.
    pub fn picked_rows(&self) -> Vec<usize> {
        let picks = self.picks.borrow();
        if picks.is_empty() {
            return Vec::new();
        }
        let entries = self.entries.borrow();
        entries
            .iter()
            .enumerate()
            .filter(|&(_, entry)| picks.contains(entry))
            .map(|(row, _)| row)
            .collect()
    }

    /// How many rows are picked, and their tracks in queue order, each track once. The row menu
    /// labels its batch entries with the count and hands them the ids, so a track picked in two
    /// rows is removed twice and goes into a playlist once.
    pub fn picked_tracks(&self) -> (usize, Vec<i32>) {
        let picks = self.picks.borrow();
        if picks.is_empty() {
            return (0, Vec::new());
        }
        let ids = self.store.ids();
        let entries = self.entries.borrow();
        let mut held = HashSet::with_capacity(picks.len());
        let mut tracks = Vec::with_capacity(picks.len());
        let mut count = 0;
        for (&id, entry) in ids.iter().zip(entries.iter()) {
            if !picks.contains(entry) {
                continue;
            }
            count += 1;
            let id = clamp_i64_to_i32(id);
            if held.insert(id) {
                tracks.push(id);
            }
        }
        (count, tracks)
    }

    fn select(&self, picks: HashSet<QueueEntry>, anchor: Option<QueueEntry>) {
        self.anchor.set(anchor);
        let was_picked = self.picks.replace(picks);
        let entries = Arc::clone(&self.entries.borrow());
        self.repaint_changed(&entries, &was_picked);
    }

    /// Keep the picks and the anchor whose entries are still queued, after the entries moved from
    /// `was_entries`.
    fn keep_queued_picks(&self, was_entries: &[QueueEntry]) {
        let anchor = self.anchor.get();
        if self.picks.borrow().is_empty() && anchor.is_none() {
            return;
        }
        let mut kept = HashSet::new();
        let mut anchor_queued = false;
        {
            let entries = self.entries.borrow();
            let picks = self.picks.borrow();
            for &entry in entries.iter() {
                anchor_queued |= anchor == Some(entry);
                if picks.contains(&entry) {
                    kept.insert(entry);
                }
            }
        }
        if !anchor_queued {
            self.anchor.set(None);
        }
        let was_picked = self.picks.replace(kept);
        self.repaint_changed(was_entries, &was_picked);
    }

    /// Repaint every row whose flag differs from what `was_entries` under `was_picked` stamped.
    /// The positions are collected first, so no borrow is held when Slint comes back for a row.
    fn repaint_changed(&self, was_entries: &[QueueEntry], was_picked: &HashSet<QueueEntry>) {
        let changed: Vec<usize> = {
            let entries = self.entries.borrow();
            let picks = self.picks.borrow();
            entries
                .iter()
                .enumerate()
                .filter(|&(row, entry)| {
                    let was = was_entries.get(row).is_some_and(|was| was_picked.contains(was));
                    picks.contains(entry) != was
                })
                .map(|(row, _)| row)
                .collect()
        };
        for row in changed {
            self.store.notifier().row_changed(row);
        }
    }
}

impl Model for QueueRows {
    type Data = QueueRow;

    fn row_count(&self) -> usize {
        self.store.row_count()
    }

    fn row_data(&self, row: usize) -> Option<QueueRow> {
        let mut data = self.store.row_data(row)?;
        let picks = self.picks.borrow();
        data.selected =
            !picks.is_empty() && self.entry_at(row).is_some_and(|entry| picks.contains(&entry));
        Some(data)
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        self.store.model_tracker()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PagedRow for QueueRow {
    // Blank but live: a row clicked before its fetch lands still skips to and removes by
    // position, and picks by its entry, which the panel holds whatever the fetch.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }
}

/// Fetches the rows the queue panel asks for.
pub fn fetcher(state: &AppState) -> impl Fn(RowRequest<QueueRow>) + 'static {
    fetch_by_id(state, convert)
}

/// The queue's `ids` with the rows of the [`window`] around `at`, so the panel opens on text where
/// it seats the current track rather than on placeholders.
pub async fn seed(state: &AppState, ids: Arc<[i64]>, at: usize) -> AppResult<Keyset<QueueRow>> {
    let read = ReadStamp::now();
    let seed =
        convert(&library::keyset::rows_by_ids(&state.db, &ids[window(ids.len(), at)]).await?);
    Ok(Keyset { ids, seed, read })
}

/// The rows of one fetch, sharing a pool so an album's tracks share their strings.
pub fn convert(rows: &[RsTrackListRow]) -> Vec<(i64, QueueRow)> {
    let mut pool = StringPool::default();
    rows.iter().map(|row| (row.id, to_queue_row(row, &mut pool))).collect()
}

/// One track as a queue row, unselected. Carries no cover: the panel and Up Next each resolve
/// their own through their `request-cover`, which is what lets them share this while reading
/// different cover tiers.
pub fn to_queue_row(row: &RsTrackListRow, pool: &mut StringPool) -> QueueRow {
    QueueRow {
        id: clamp_i64_to_i32(row.id),
        title: row.title.as_str().into(),
        artist: pool.intern(row.artist.as_deref().unwrap_or_default()),
        artwork_path: pool.intern(row.artwork_path.as_deref().unwrap_or_default()),
        display_duration: pool.intern(&format_duration_ms(row.duration_ms.max(0))),
        selected: false,
        is_favorite: row.is_favorite,
        rating: row.rating,
        album_id: row.album_id.map_or(0, clamp_i64_to_i32),
        artist_id: row.artist_id.map_or(0, clamp_i64_to_i32),
        genre_id: row.genre_id.map_or(0, clamp_i64_to_i32),
    }
}

#[cfg(test)]
#[path = "tests/queue_rows_tests.rs"]
mod tests;
