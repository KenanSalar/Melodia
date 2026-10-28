//! The Slint model for a list that can hold the whole library.
//!
//! A list keeps its ordered ids, the keyset, and only the rows near what Slint last read, fetching
//! the rest by id as the viewport reaches them. An open list costs eight bytes a track plus a few
//! hundred rows however large the library is: its sort and its filter are the keyset query's, and
//! a fetch is a lookup by primary key.
//!
//! **Rows are cached by id, not by position.** What a row reads doesn't depend on where it sits,
//! so a new sort, a narrower filter or a removal keeps every cached row valid and paints with no
//! placeholder. Only a library change can make one stale, which [`PagedRows::reread`] answers.
//!
//! **One fetch is out at a time, and it asks for what the viewport lacks when it is sent.** A read
//! that misses, or one onto another page, asks for the rows missing around the row Slint read
//! last, nearest first, and each landing asks again from wherever the list has got to since. A fast
//! drag fetches where it stops rather than every page it passed, and a list holds one connection
//! however hard it is scrolled.
//!
//! A row not fetched yet reads as [`PagedRow::placeholder`], and the rows a fetch changes are
//! repainted as it lands. `selected` is the model's own, stamped on cached and placeholder rows
//! alike: a row reads its neighbours' flags to join a run of picks, and those may not have landed.
//!
//! UI thread only. Nothing may walk one with `Model::iter`, which reads every row and drags the
//! window across the whole list.
//!
//! A card grid keeps one too, as the store under the chunked rows [`grid::PagedGrid`] hands
//! Slint; where a store's changes go is its [`RowNotify`].

pub mod grid;
pub mod queue_rows;
pub mod track_rows;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::hash::BuildHasher;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use lru::LruCache;
use parking_lot::Mutex;
use slint::{Model, ModelNotify, ModelRc, ModelTracker};
use tokio::runtime::Handle;

use crate::ui::util::{self, clamp_i64_to_i32};
use melodia_core::error::{AppResult, describe};
use melodia_core::utils::block_move;
use melodia_core::utils::needle::Needle;

/// The step the window moves by. A list asks for more only when Slint reads onto another page, so
/// scrolling row by row fetches a page at a time rather than a row.
const PAGE_ROWS: usize = 64;

/// Pages kept fetched either side of the one Slint reads from, so ordinary scrolling finds its
/// rows already there. The window is wider than any screen of rows, which is what lets a landing
/// repaint only inside it: Slint builds rows for the screen alone.
const PREFETCH_PAGES: usize = 2;

/// Rows per fetch: enough for the page Slint reads and the nearer of its neighbours, so a jump
/// paints in one round trip whichever end of the screen was read last.
const FETCH_ROWS: usize = PAGE_ROWS * 2;

/// How many rows stay cached. Android Paging's floor is `2 × prefetch + page`, five pages here;
/// eight keeps the rows on screen clear of what a scroll's prefetch evicts.
const ROW_CAP: NonZeroUsize = match NonZeroUsize::new(PAGE_ROWS * 8) {
    Some(cap) => cap,
    None => panic!("ROW_CAP > 0"),
};

/// The positions a list keeps fetched while Slint reads `row`: its page and [`PREFETCH_PAGES`]
/// either side, within `len`. A load seeds the same span, so a list shown where it was scrolled
/// finds every row it reads already there.
pub fn window(len: usize, row: usize) -> Range<usize> {
    if len == 0 {
        return 0..0;
    }
    let page = row.min(len - 1) / PAGE_ROWS;
    let start = page.saturating_sub(PREFETCH_PAGES) * PAGE_ROWS;
    let end = ((page + PREFETCH_PAGES + 1) * PAGE_ROWS).min(len);
    start..end
}

/// The positions of `span` by distance from `row`, which lies inside it, nearer first.
fn nearest_first(span: Range<usize>, row: usize) -> impl Iterator<Item = usize> {
    (0..span.len())
        .flat_map(move |d| [row.checked_add(d), row.checked_sub(d + 1)])
        .flatten()
        .filter(move |pos| span.contains(pos))
}

/// A row a [`PagedRows`] can show.
pub trait PagedRow: Clone + PartialEq + 'static {
    /// The row shown for `id` until its fetch lands.
    fn placeholder(id: i64) -> Self;

    /// Stamps the list's own pick on the row. A card picks through `CardSelection` instead, so a
    /// card keeps this default.
    fn set_selected(&mut self, _selected: bool) {}
}

/// Where a [`PagedRows`] reports that its rows changed: the `ModelNotify` a flat list hands
/// Slint, or a grid's chunks.
pub trait RowNotify: 'static {
    fn reset(&self);
    fn row_changed(&self, row: usize);
    fn row_added(&self, index: usize, count: usize);
    fn row_removed(&self, index: usize, count: usize);
}

impl RowNotify for ModelNotify {
    fn reset(&self) {
        ModelNotify::reset(self);
    }

    fn row_changed(&self, row: usize) {
        ModelNotify::row_changed(self, row);
    }

    fn row_added(&self, index: usize, count: usize) {
        ModelNotify::row_added(self, index, count);
    }

    fn row_removed(&self, index: usize, count: usize) {
        ModelNotify::row_removed(self, index, count);
    }
}

static READS: AtomicU64 = AtomicU64::new(0);

/// Where a read falls among every read and patch any list stores, on one counter all threads
/// share. A row read earlier than the one a list already holds never replaces it, so a fetch sent
/// before a patch can't revert the patch when it lands after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReadStamp(u64);

impl ReadStamp {
    /// Taken before the query whose rows it will stamp.
    pub fn now() -> Self {
        Self(READS.fetch_add(1, Ordering::AcqRel))
    }
}

/// The row a list was last read around, readable from any thread, so a load running off the event
/// loop seeds the rows where the list is scrolled: Slint keeps a list's scroll offset across a
/// reset, so a load seeding the top paints the screen on placeholders.
#[derive(Clone, Default)]
pub struct ScrollPoint(Arc<AtomicUsize>);

impl ScrollPoint {
    pub fn row(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

/// A list's ordered ids and the rows a load fetched with them, the [`window`] it opens on.
pub struct Keyset<R> {
    pub ids: Arc<[i64]>,
    pub seed: Vec<(i64, R)>,
    pub read: ReadStamp,
}

/// The count a header states whatever the filter leaves: `ids`' own length unfiltered, else
/// `count`'s answer.
pub async fn unfiltered_total(
    ids: &[i64],
    needle: &Needle,
    count: impl Future<Output = AppResult<i64>>,
) -> AppResult<i32> {
    let total = if needle.is_empty() {
        i64::try_from(ids.len()).unwrap_or(i64::MAX)
    } else {
        count.await?
    };
    Ok(i32::try_from(total).unwrap_or(i32::MAX))
}

struct Cached<R> {
    row: R,
    read: ReadStamp,
}

pub struct PagedRows<R: PagedRow, N: RowNotify = ModelNotify> {
    keyset: RefCell<Arc<[i64]>>,
    rows: RefCell<LruCache<i64, Cached<R>>>,
    /// Whether a fetch is out. Only one ever is, so nothing needs to say which ids it asked for.
    fetching: Cell<bool>,
    /// Ids a fetch asked for and didn't get back, deleted since the keyset was read. None is asked
    /// for again before the next keyset, so a row gone from the database can't be fetched in a loop.
    absent: RefCell<HashSet<i64>>,
    selected: RefCell<HashSet<i32>>,
    /// When the keyset on show was read.
    shown_read: Cell<ReadStamp>,
    /// A cached row read before this predates the library change [`Self::reread`] answered, and is
    /// fetched again.
    stale_before: Cell<ReadStamp>,
    /// The row Slint last read, which the window centres on. `None` until a list emptied by
    /// [`Self::clear`] is read again.
    anchor: Cell<Option<usize>>,
    scroll: ScrollPoint,
    fetch: Box<dyn Fn(RowRequest<R>)>,
    this: Weak<Self>,
    notify: N,
}

impl<R: PagedRow> PagedRows<R> {
    /// An empty list whose rows come from `fetch`. A request it drops without landing frees the
    /// list to send another, which it does when it is next read.
    pub fn new(fetch: impl Fn(RowRequest<R>) + 'static) -> Rc<Self> {
        Self::with_notify(fetch, ModelNotify::default())
    }

    /// The model behind `rows`, where it is one.
    pub fn of(rows: &ModelRc<R>) -> Option<&Self> {
        rows.as_any().downcast_ref()
    }

    /// [`Self::of`] for a list installed with one, logging a miss under `label`
    /// ([`util::installed`]).
    pub fn installed<'a>(rows: &'a ModelRc<R>, label: &str) -> Option<&'a Self> {
        util::installed(rows, label)
    }

    /// The ids `rows` shows, in order, for a play or a shuffle. None where it isn't one.
    pub fn ids_of(rows: &ModelRc<R>, label: &str) -> Vec<i64> {
        Self::installed(rows, label).map(|model| model.ids().to_vec()).unwrap_or_default()
    }
}

impl<R: PagedRow, N: RowNotify> PagedRows<R, N> {
    /// [`PagedRows::new`] reporting its changes to `notify`.
    pub fn with_notify(fetch: impl Fn(RowRequest<R>) + 'static, notify: N) -> Rc<Self> {
        Rc::new_cyclic(|this| Self {
            keyset: RefCell::new(Arc::from([])),
            rows: RefCell::new(LruCache::new(ROW_CAP)),
            fetching: Cell::new(false),
            absent: RefCell::default(),
            selected: RefCell::default(),
            shown_read: Cell::new(ReadStamp(0)),
            stale_before: Cell::new(ReadStamp(0)),
            anchor: Cell::new(None),
            scroll: ScrollPoint::default(),
            fetch: Box::new(fetch),
            this: this.clone(),
            notify,
        })
    }

    /// Where this list was last read, for the loads that seed it.
    pub fn scroll_point(&self) -> ScrollPoint {
        self.scroll.clone()
    }

    /// Show `keyset`, its seed rows stored first. The same ids in the same order repaint only the
    /// rows that read differently, so a library tick that moved nothing on screen rebuilds no
    /// delegate; anything else resets. The incoming ids are kept either way, so a reorder shown
    /// ahead of its write shares the publisher's copy once that lands. The anchor survives the
    /// reset as Slint's scroll offset does, which is the span the seed was fetched for.
    pub fn show(&self, keyset: Keyset<R>) {
        let Keyset { ids, seed, read } = keyset;
        self.shown_read.set(read);
        self.absent.borrow_mut().clear();
        let changed = self.store(seed, read);
        let same = **self.keyset.borrow() == *ids;
        *self.keyset.borrow_mut() = ids;
        if same {
            self.repaint(&changed);
            return;
        }
        self.notify.reset();
    }

    /// Fetch again every cached row read before the keyset on show, after a library change that
    /// may have rewritten any of them: those around the viewport first, the rest once the window is
    /// whole. Each is repainted only where it lands reading differently.
    pub fn reread(&self) {
        self.stale_before.set(self.shown_read.get());
        self.pump();
    }

    /// Edit the cached row for `id`, after a write the database has already taken, and repaint
    /// it. A row not cached has nothing to edit: its next fetch reads the write.
    pub fn patch(&self, id: i64, edit: impl FnOnce(&mut R)) {
        let patched = match self.rows.borrow_mut().peek_mut(&id) {
            Some(cached) => {
                edit(&mut cached.row);
                cached.read = ReadStamp::now();
                true
            }
            None => false,
        };
        if patched {
            self.repaint(&HashSet::from([id]));
        }
    }

    /// Drop `ids` from the list. A single row leaves on its own, which keeps every other row where
    /// it is on screen; more than one resets.
    pub fn remove<S: BuildHasher>(&self, ids: &HashSet<i64, S>) {
        let removed: Vec<usize> = self
            .keyset
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, id)| ids.contains(id))
            .map(|(pos, _)| pos)
            .collect();
        if removed.is_empty() {
            return;
        }
        let rest: Arc<[i64]> =
            self.keyset.borrow().iter().filter(|id| !ids.contains(id)).copied().collect();
        *self.keyset.borrow_mut() = rest;
        if let [at] = removed.as_slice() {
            self.notify.row_removed(*at, 1);
        } else {
            self.notify.reset();
        }
    }

    /// Move the rows at `rows` together to `to`, counted once they have left, which is where a
    /// drag drops them ([`block_move::move_block`]). Only those rows leave and arrive, so every
    /// other row keeps its delegate. Answers whether anything moved.
    pub fn move_rows(&self, rows: &[usize], to: usize) -> bool {
        let mut ids = self.keyset.borrow().to_vec();
        if !block_move::move_block(&mut ids, rows, to) {
            return false;
        }
        *self.keyset.borrow_mut() = Arc::from(ids);
        for &row in rows.iter().rev() {
            self.notify.row_removed(row, 1);
        }
        self.notify.row_added(to, rows.len());
        true
    }

    /// Empty the list and its cache. The selection stays, as `selected-ids` does.
    pub fn clear(&self) {
        *self.keyset.borrow_mut() = Arc::from([]);
        self.rows.borrow_mut().clear();
        self.absent.borrow_mut().clear();
        self.set_anchor(None);
        self.notify.reset();
    }

    pub fn ids(&self) -> Arc<[i64]> {
        Arc::clone(&self.keyset.borrow())
    }

    /// The cached row for `id`, without fetching or moving the window: what a callback acting on
    /// a row on screen reads it by.
    pub fn cached(&self, id: i64) -> Option<R> {
        self.rows.borrow().peek(&id).map(|cached| cached.row.clone())
    }

    fn len(&self) -> usize {
        self.keyset.borrow().len()
    }

    fn notifier(&self) -> &N {
        &self.notify
    }

    /// Every id the list shows, in order, as the selection names them.
    pub fn row_ids(&self) -> Vec<i32> {
        self.keyset.borrow().iter().map(|&id| clamp_i64_to_i32(id)).collect()
    }

    /// Make `ids` the selected set, repainting the rows whose flag moved.
    pub fn select<S: BuildHasher>(&self, ids: &HashSet<i32, S>) {
        let moved: Vec<usize> = {
            let selected = self.selected.borrow();
            self.keyset
                .borrow()
                .iter()
                .map(|&id| clamp_i64_to_i32(id))
                .enumerate()
                .filter(|(_, id)| selected.contains(id) != ids.contains(id))
                .map(|(pos, _)| pos)
                .collect()
        };
        *self.selected.borrow_mut() = ids.iter().copied().collect();
        for pos in moved {
            self.notify.row_changed(pos);
        }
    }

    /// Where the selected rows sit, in order.
    pub fn picked_rows(&self) -> Vec<usize> {
        let selected = self.selected.borrow();
        self.keyset
            .borrow()
            .iter()
            .enumerate()
            .filter(|&(_, &id)| selected.contains(&clamp_i64_to_i32(id)))
            .map(|(row, _)| row)
            .collect()
    }

    /// Select every row, returning their ids in order.
    pub fn select_all(&self) -> Vec<i32> {
        let ids = self.row_ids();
        self.select(&ids.iter().copied().collect::<HashSet<i32>>());
        ids
    }

    /// Move the anchor, returning where it was, and publish it for the loads.
    fn set_anchor(&self, row: Option<usize>) -> Option<usize> {
        self.scroll.0.store(row.unwrap_or(0), Ordering::Relaxed);
        self.anchor.replace(row)
    }

    /// Send a fetch for what the list wants next, unless one is out: that one asks again as it
    /// lands, from wherever the list has got to by then.
    fn pump(&self) {
        if self.fetching.get() {
            return;
        }
        let ids = self.wanted();
        if ids.is_empty() {
            return;
        }
        self.fetching.set(true);
        let model: Weak<dyn Landing<R>> = self.this.clone();
        // No borrow is held here: a fetcher that drops the request on the spot runs its `Drop`,
        // which comes straight back for `fetching`.
        (self.fetch)(RowRequest { ids, read: ReadStamp::now(), model });
    }

    /// The rows the window lacks or holds stale, nearest the anchor first, and failing those the
    /// stale rows anywhere else in the cache, which the viewport may come back to.
    fn wanted(&self) -> Vec<i64> {
        let keyset = self.keyset.borrow();
        let rows = self.rows.borrow();
        let absent = self.absent.borrow();
        let stale_before = self.stale_before.get();
        let stale = |cached: &Cached<R>| cached.read < stale_before;

        if let Some(anchor) = self.anchor.get()
            && !keyset.is_empty()
        {
            let row = anchor.min(keyset.len() - 1);
            let near: Vec<i64> = nearest_first(window(keyset.len(), row), row)
                .map(|pos| keyset[pos])
                .filter(|id| !absent.contains(id) && rows.peek(id).is_none_or(stale))
                .take(FETCH_ROWS)
                .collect();
            if !near.is_empty() {
                return near;
            }
        }
        rows.iter()
            .filter(|&(id, cached)| stale(cached) && !absent.contains(id))
            .map(|(id, _)| *id)
            .take(FETCH_ROWS)
            .collect()
    }

    fn land(&self, asked: &[i64], read: ReadStamp, rows: Vec<(i64, R)>) {
        self.fetching.set(false);
        // A cleared list keeps nothing a fetch sent before the clear brings.
        if self.keyset.borrow().is_empty() {
            return;
        }
        let returned: HashSet<i64> = rows.iter().map(|(id, _)| *id).collect();
        self.absent.borrow_mut().extend(asked.iter().filter(|id| !returned.contains(id)));
        let changed = self.store(rows, read);
        self.repaint(&changed);
        self.pump();
    }

    /// A fetch that failed or was never spawned. Nothing is retried until the list is next read,
    /// so a database refusing every fetch costs one per scroll rather than one per landing.
    fn abandon(&self) {
        self.fetching.set(false);
    }

    /// Store `rows`, read at `read`, returning the ids that now read differently. A cached row
    /// read later than `read` is kept.
    fn store(&self, rows: Vec<(i64, R)>, read: ReadStamp) -> HashSet<i64> {
        let mut cache = self.rows.borrow_mut();
        let mut changed = HashSet::new();
        for (id, row) in rows {
            match cache.peek_mut(&id) {
                Some(cached) if cached.read > read => {}
                Some(cached) if cached.row == row => cached.read = read,
                _ => {
                    cache.put(id, Cached { row, read });
                    changed.insert(id);
                }
            }
        }
        changed
    }

    /// Ask Slint to read again the positions in the window showing one of `ids`, and no others:
    /// Slint holds rows for the screen alone, which the window spans, and a row it doesn't hold
    /// reads the cache when it next builds one.
    fn repaint(&self, ids: &HashSet<i64>) {
        let Some(anchor) = self.anchor.get() else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        let positions: Vec<usize> = {
            let keyset = self.keyset.borrow();
            window(keyset.len(), anchor).filter(|&pos| ids.contains(&keyset[pos])).collect()
        };
        for pos in positions {
            self.notify.row_changed(pos);
        }
    }

    /// The row at `row` as Slint reads it, which is also what moves the window there.
    fn read(&self, row: usize) -> Option<R> {
        let id = *self.keyset.borrow().get(row)?;
        let was = self.set_anchor(Some(row));
        let new_page = was.map(|at| at / PAGE_ROWS) != Some(row / PAGE_ROWS);
        let cached = self.rows.borrow_mut().get(&id).map(|cached| {
            let stale = cached.read < self.stale_before.get();
            (cached.row.clone(), stale)
        });
        let wants_fetch = new_page || cached.as_ref().is_none_or(|(_, stale)| *stale);
        let mut data = cached.map_or_else(|| R::placeholder(id), |(row, _)| row);
        data.set_selected(self.selected.borrow().contains(&clamp_i64_to_i32(id)));
        if wants_fetch {
            self.pump();
        }
        Some(data)
    }
}

impl<R: PagedRow> Model for PagedRows<R> {
    type Data = R;

    fn row_count(&self) -> usize {
        self.len()
    }

    fn row_data(&self, row: usize) -> Option<R> {
        self.read(row)
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Where a [`RowRequest`] lands, whatever its list reports to.
trait Landing<R> {
    fn land(&self, asked: &[i64], read: ReadStamp, rows: Vec<(i64, R)>);
    fn abandon(&self);
}

impl<R: PagedRow, N: RowNotify> Landing<R> for PagedRows<R, N> {
    fn land(&self, asked: &[i64], read: ReadStamp, rows: Vec<(i64, R)>) {
        PagedRows::land(self, asked, read, rows);
    }

    fn abandon(&self) {
        PagedRows::abandon(self);
    }
}

/// The rows a list asked for, and where they go when they arrive. It holds the model weakly and
/// is not `Send`: a fetcher lands it on the UI thread or drops it there, and either frees the list
/// to send its next.
pub struct RowRequest<R: PagedRow> {
    ids: Vec<i64>,
    read: ReadStamp,
    model: Weak<dyn Landing<R>>,
}

impl<R: PagedRow> RowRequest<R> {
    pub fn ids(&self) -> &[i64] {
        &self.ids
    }

    /// Hand the rows over, in any order and with any of them missing: a row deleted since the
    /// keyset was read stays a placeholder until the library tick that deleted it reloads the
    /// keyset without it, and isn't asked for again before then.
    pub fn land(mut self, rows: Vec<(i64, R)>) {
        let asked = std::mem::take(&mut self.ids);
        if let Some(model) = self.model.upgrade() {
            model.land(&asked, self.read, rows);
        }
    }
}

/// Runs `query` on the runtime and hands its answer to `land` on the UI thread, logging a failure
/// under `label`. The query is spawned rather than awaited on the event loop, so its connection
/// goes back to the pool as soon as the rows are read however busy the loop is drawing; only the
/// landing waits for it. A request a failure drops frees its list to ask again.
pub fn spawn_fetch<R: PagedRow, T: Send + 'static>(
    runtime: &Handle,
    request: RowRequest<R>,
    query: impl Future<Output = AppResult<T>> + Send + 'static,
    land: impl FnOnce(RowRequest<R>, T) + 'static,
    label: &'static str,
) {
    let answer = runtime.spawn(query);
    let spawned = slint::spawn_local(async_compat::Compat::new(async move {
        match answer.await {
            Ok(Ok(answer)) => land(request, answer),
            Ok(Err(e)) => log::warn!("{label}: fetch failed: {}", describe(&e)),
            Err(e) => log::warn!("{label}: fetch task failed: {}", describe(&e)),
        }
    }));
    if let Err(e) = spawned {
        log::warn!("{label}: fetch not spawned: {}", describe(&e));
    }
}

impl<R: PagedRow> Drop for RowRequest<R> {
    fn drop(&mut self) {
        // Emptied by `land`; anything still here never landed.
        if self.ids.is_empty() {
            return;
        }
        if let Some(model) = self.model.upgrade() {
            model.abandon();
        }
    }
}

/// Orders a list's keyset loads, so whichever was asked for last is the one shown, however they
/// land. A load takes its ticket before reading the sort and the filter it runs under, and shows
/// only while that ticket is still the latest.
///
/// A reload, what a library change asks for, also re-reads the rows the list holds, and a sort or
/// keystroke overtaking it would drop that re-read with it. So a reload stays owed until one
/// shows, and a requery issued meanwhile runs as one. Deciding that and issuing the ticket are one
/// step under the lock, or a reload issued on another thread between the two is overtaken anyway.
#[derive(Default)]
pub struct KeysetTickets(Mutex<Issued>);

#[derive(Default)]
struct Issued {
    latest: u64,
    /// The newest reload no load has shown yet.
    reload_owed: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(u64);

/// What a sort or a filter has to run as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// The ids alone, every row the list holds being current.
    Requery(Ticket),
    /// A reload, standing in for one this load overtakes.
    Reload(Ticket),
}

impl KeysetTickets {
    /// The ticket for a reload, owed until [`Self::reload_shown`] hands it back.
    pub fn issue_reload(&self) -> Ticket {
        let mut issued = self.0.lock();
        issued.latest += 1;
        issued.reload_owed = Some(issued.latest);
        Ticket(issued.latest)
    }

    /// The ticket for a sort or a filter, which takes over a reload still owed.
    pub fn issue_requery(&self) -> Load {
        let mut issued = self.0.lock();
        issued.latest += 1;
        let ticket = Ticket(issued.latest);
        if issued.reload_owed.is_none() {
            return Load::Requery(ticket);
        }
        issued.reload_owed = Some(ticket.0);
        Load::Reload(ticket)
    }

    pub fn is_latest(&self, ticket: Ticket) -> bool {
        self.0.lock().latest == ticket.0
    }

    /// Settle the reload `ticket` showed. One issued after it stays owed.
    pub fn reload_shown(&self, ticket: Ticket) {
        let mut issued = self.0.lock();
        if issued.reload_owed == Some(ticket.0) {
            issued.reload_owed = None;
        }
    }

    /// Refuse every load issued so far, for a list whose rows were just released: nothing they
    /// fetched has anywhere to go, and the list's next load seats a fresh keyset, so no reload
    /// stays owed either.
    pub fn revoke(&self) {
        let mut issued = self.0.lock();
        issued.latest += 1;
        issued.reload_owed = None;
    }
}

#[cfg(test)]
#[path = "tests/paged_rows_tests.rs"]
mod tests;
