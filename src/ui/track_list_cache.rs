//! The retained row set behind a filterable, sortable track list.
//!
//! Two views keep an entire track list resident so a keystroke or a header click costs no
//! DB round trip: My Library's Songs tab, holding the whole library, and Favorites'.
//!
//! The cache holds **already-converted** rows rather than DB ones. `SharedString` is
//! refcounted, so handing a cached row to the model is a pointer copy and a few atomic
//! increments where `SharedString::from(&str)` allocates — the display text exists once,
//! a filter keystroke allocates nothing per visible row, and the single-row patches can't
//! deep-clone a library's worth of `String`s.
//!
//! What a DB row still answers and a converted one cannot is the sort: `disc_number` never
//! reaches the UI and `sort_key` is not a displayed column. Those two plus the untruncated
//! `i64` id are [`TrackSortKey`]; [`SortRow`] is the pair viewed as one thing.
//!
//! **Alignment is the invariant, and one lock is how it is kept.** `rows`, `search` and
//! `sort` are positionally aligned and `order` is a permutation of their indices, so all
//! four live in one [`CacheData`] behind one `Mutex<Arc<…>>`. **All four are `Arc`s
//! *inside* it**, so cloning bumps four refcounts rather than duplicating a `Box<str>` per
//! row; `rows` is the only one ever patched and holds its own, so a re-sort — replacing
//! `order` alone — stops paying for the row copy `Arc::make_mut` would make for it.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::TrackListRow as UiTrackListRow;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::ui::row_match::{self, Needle};
use crate::ui::track_sort::{self, TrackSortFields};
use crate::ui::tracks::to_slint_track_list_row;
use crate::ui::util::len_as_i32;

/// Pre-folded text columns for filtering: one packed `Box<str>` of
/// [`row_match::search_fields`] joined by `\0`, so one heap allocation per row rather than
/// a `String` header per column. `\0` is safe as a separator — needles come from text
/// input and `push_folded` maps away the rare field value carrying one.
///
/// `year` stays an integer beside the packed text, so this and every `track_matches`
/// surface run the *same* [`Needle::matches_number`] rule rather than two spellings of it.
///
/// `Clone` is for [`TrackListCache::remove`] alone — the only operation that changes the
/// set's length, and so the only one that can't leave a reader's snapshot pointing at the
/// vector it is shortening.
#[derive(Clone)]
pub struct RowSearchKey {
    packed: Box<str>,
    year: Option<i32>,
}

impl RowSearchKey {
    pub fn from_row(r: &RsTrackListRow) -> Self {
        let fields = row_match::search_fields(r);
        let text_len: usize = fields.iter().map(|f| f.len()).sum();
        // One separator per pair, so one fewer than there are fields.
        let mut buf = String::with_capacity(text_len + fields.len() - 1);
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                buf.push('\0');
            }
            row_match::push_folded(&mut buf, field);
        }
        Self {
            packed: buf.into_boxed_str(),
            year: r.year,
        }
    }

    /// A plain `str::contains` on the packed text, both sides already folded — the one
    /// matcher that doesn't go through [`Needle::contains`], and what `Needle::as_str`
    /// exists for.
    pub fn matches(&self, needle: &Needle) -> bool {
        self.packed.contains(needle.as_str()) || needle.matches_number(self.year)
    }
}

/// What a sort needs and a converted display row cannot answer: `id` because the UI row's
/// is clamped to `i32` while the play and queue paths take `i64`, `sort_key` and `disc`
/// because neither reaches a cell. `disc` is stored already flattened onto the
/// `TrackSortFields` sentinel; `sort_key` stays raw, the comparator owning the case fold
/// exactly as it does for a DB row. `Clone` for [`TrackListCache::remove`], as
/// [`RowSearchKey`].
#[derive(Clone)]
struct TrackSortKey {
    id: i64,
    sort_key: Box<str>,
    disc: i32,
}

/// A cached row and its sort key viewed as one comparable thing, because
/// [`track_sort::sort_track_rows_by`] projects each element to a single
/// `&impl TrackSortFields` and here the fields are split across two parallel `Vec`s.
struct SortRow<'a> {
    row: &'a UiTrackListRow,
    key: &'a TrackSortKey,
}

/// Every conversion here undoes one [`crate::ui::tracks::prepare_track_list_row`] made, so
/// this produces the order the DB rows produce — `the_cache_sorts_exactly_as_the_db_rows_do`
/// holds it to that. `track_number` is the one needing work: it arrives `unwrap_or(0)` and
/// the sort's arm reads `0` as "unnumbered", so the sentinel has to be rebuilt or an
/// untracked row sorts first. A `0` and a missing `year` stay indistinguishable, which
/// `the_cache_conflates_a_zero_year_with_a_missing_one` records.
impl TrackSortFields for SortRow<'_> {
    fn disc(&self) -> i32 {
        self.key.disc
    }

    fn track(&self) -> i64 {
        match self.row.track_number {
            n if n != 0 => i64::from(n),
            _ => i64::from(i32::MAX),
        }
    }

    fn artist(&self) -> &str {
        &self.row.artist
    }

    fn album(&self) -> &str {
        &self.row.album
    }

    fn genre(&self) -> &str {
        &self.row.genre
    }

    fn year(&self) -> Option<i32> {
        Some(self.row.year)
    }

    fn duration_ms(&self) -> i64 {
        i64::from(self.row.duration_ms)
    }

    fn sort_key(&self) -> &str {
        &self.key.sort_key
    }
}

/// The four aligned vectors, swapped as one so a reader can never see a half-updated set.
/// Cloning is the copy-on-write step behind every single-row patch: one `Vec` allocation
/// plus a refcount bump per `SharedString` and per inner `Arc`, no string data copied.
#[derive(Clone)]
pub struct CacheData {
    /// Display rows in fetch order — never reordered, only patched in place. Behind its
    /// own `Arc` because `Arc::make_mut` on the enclosing [`CacheData`] clones **every**
    /// field it finds shared, so a [`TrackListCache::resort`] landing while a background
    /// fetch held a snapshot duplicated the whole row vector to move a permutation.
    rows: Arc<Vec<UiTrackListRow>>,
    /// Filter keys, aligned with `rows`.
    search: Arc<Vec<RowSearchKey>>,
    /// Sort keys, aligned with `rows`.
    sort: Arc<Vec<TrackSortKey>>,
    /// Display-order permutation of `0..rows.len()`.
    order: Arc<Vec<usize>>,
}

impl CacheData {
    fn empty() -> Self {
        Self {
            rows: Arc::new(Vec::new()),
            search: Arc::new(Vec::new()),
            sort: Arc::new(Vec::new()),
            order: Arc::new(Vec::new()),
        }
    }

    /// Rows passing `needle`, in display order, ready for the Slint model. Each is a
    /// `clone` of a cached row — a struct copy plus one atomic increment per
    /// `SharedString`, no allocation. Runs per throttled keystroke.
    pub fn visible(&self, needle: &Needle) -> Vec<UiTrackListRow> {
        let mut out = Vec::with_capacity(self.reserve_for(needle));
        out.extend(self.walk(needle).map(|(_, row)| row.clone()));
        out
    }

    /// Ids of the rows passing `needle`, in display order — what a row activation hands
    /// to `player_play_tracks` so the queue becomes the list the user is looking at.
    pub fn ids_filtered(&self, needle: &Needle) -> Vec<i64> {
        let mut out = Vec::with_capacity(self.reserve_for(needle));
        out.extend(self.walk(needle).map(|(i, _)| self.sort[i].id));
        out
    }

    /// Exact capacity for an unfiltered walk, none for a filtered one. A `Filter` adapter
    /// reports a `size_hint` lower bound of `0`, so building straight off [`Self::walk`]
    /// grows geometrically — and the empty needle is the case every cold fetch and every
    /// library tick takes over the largest list in the app. A real needle gets nothing:
    /// nothing cheap predicts the survivor count, and reserving a library-sized `Vec` to
    /// put three rows in is the worse wrong.
    fn reserve_for(&self, needle: &Needle) -> usize {
        if needle.is_empty() {
            self.rows.len()
        } else {
            0
        }
    }

    /// Unique artwork paths in **display** order, capped, so that on a library with more
    /// unique covers than the tier holds, what survives the cap is what is seen first.
    pub fn artwork_paths(&self, cap: usize) -> Vec<PathBuf> {
        crate::ui::grid_prewarm::unique_artwork_paths(
            self.order.iter().map(|&i| Some(self.rows[i].artwork_path.as_str())),
            cap,
        )
    }

    /// Number of rows before filtering — the view's `total-count`.
    pub fn total(&self) -> i32 {
        len_as_i32(self.rows.len())
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The one walk `visible` and `ids_filtered` share: display order, filtered, each
    /// surviving row beside its cache index. Indexing rather than `.get()` is safe
    /// because the four vectors share a lock — `order` is built from these very rows and
    /// replaced with them, so no concurrent fetch can leave an index past its set.
    fn walk<'a>(&'a self, needle: &'a Needle) -> impl Iterator<Item = (usize, &'a UiTrackListRow)> {
        self.order
            .iter()
            .map(|&i| (i, &self.rows[i]))
            .filter(move |(i, _)| needle.is_empty() || self.search[*i].matches(needle))
    }
}

/// A filterable, sortable track list held in memory. Cheap to snapshot and safe to read
/// from any thread; the module doc has the residency and alignment arguments.
pub struct TrackListCache {
    data: Mutex<Arc<CacheData>>,
}

impl Default for TrackListCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackListCache {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(Arc::new(CacheData::empty())),
        }
    }

    /// A consistent view of all four vectors, for a rebuild that must not hold the lock
    /// while it walks.
    pub fn snapshot(&self) -> Arc<CacheData> {
        self.data.lock().clone()
    }

    /// Convert `rows` into the cache and compute the display order for `field` / `dir`.
    pub fn store(&self, rows: Vec<RsTrackListRow>, field: &str, dir: &str) {
        let (display, search, sort) = convert(rows);
        let order = compute_order(&display, &sort, field, dir);
        self.install(display, search, sort, order);
    }

    /// [`Self::store`] for a caller that has already computed the display permutation.
    /// Favorites needs it: its cover prewarm walks the list in display order and has to run
    /// *before* the section guards allow the store, so re-deriving here would sort the set
    /// twice per fetch.
    ///
    /// `order` must be a permutation of `0..rows.len()` — the one entry point where that is
    /// a *caller's* obligation rather than a local fact, and what the bare indexing in
    /// [`CacheData::walk`] rests on. An index past the end panics on the next filter walk.
    pub fn store_in_order(&self, rows: Vec<RsTrackListRow>, order: Vec<usize>) {
        debug_assert_eq!(order.len(), rows.len(), "store_in_order needs a permutation of its rows");
        let (display, search, sort) = convert(rows);
        self.install(display, search, sort, order);
    }

    fn install(
        &self,
        rows: Vec<UiTrackListRow>,
        search: Vec<RowSearchKey>,
        sort: Vec<TrackSortKey>,
        order: Vec<usize>,
    ) {
        *self.data.lock() = Arc::new(CacheData {
            rows: Arc::new(rows),
            search: Arc::new(search),
            sort: Arc::new(sort),
            order: Arc::new(order),
        });
    }

    /// Recompute only the display permutation — a header click costs no DB round trip, no
    /// key rebuild and **no row copy either, which is what `rows` being an `Arc` buys**:
    /// `Arc::make_mut` clones every shared field, so with `rows` a bare `Vec` this
    /// duplicated the entire row set whenever a reader held a snapshot.
    ///
    /// The lock is held across [`compute_order`] deliberately: the caller is the UI thread,
    /// so this parks background workers rather than the event loop, and computing outside
    /// it would let a `store` land in the gap and leave a permutation indexing rows that no
    /// longer exist.
    pub fn resort(&self, field: &str, dir: &str) -> Arc<CacheData> {
        let mut guard = self.data.lock();
        let data = Arc::make_mut(&mut *guard);
        data.order = Arc::new(compute_order(&data.rows, &data.sort, field, dir));
        guard.clone()
    }

    /// Drop everything, for a section leave that hands its caches back.
    pub fn clear(&self) {
        *self.data.lock() = Arc::new(CacheData::empty());
    }

    /// Patch `is_favorite` on one row, so a toggle doesn't re-fetch the list and lose
    /// scroll position.
    pub fn set_favorite(&self, id: i64, fav: bool) {
        self.patch(id, |row| row.is_favorite = fav);
    }

    /// Patch `rating` on one row — the star-rating analogue of [`Self::set_favorite`].
    pub fn set_rating(&self, id: i64, rating: i32) {
        self.patch(id, |row| row.rating = rating);
    }

    /// Drop one row from all four vectors, keeping them aligned — Favorites needs it,
    /// unfavouriting removing a track from the set the list is defined by. `order` is
    /// rebuilt rather than recomputed: shifting the indices past the removed slot preserves
    /// the current sort exactly, where a re-sort needs a field and direction the caller
    /// doesn't have.
    pub fn remove(&self, id: i64) {
        let mut guard = self.data.lock();
        let Some(at) = guard.sort.iter().position(|k| k.id == id) else {
            return;
        };
        let data = Arc::make_mut(&mut *guard);
        Arc::make_mut(&mut data.rows).remove(at);
        Arc::make_mut(&mut data.search).remove(at);
        Arc::make_mut(&mut data.sort).remove(at);
        let order = Arc::make_mut(&mut data.order);
        order.retain(|&i| i != at);
        for i in order.iter_mut().filter(|i| **i > at) {
            *i -= 1;
        }
    }

    /// Copy-on-write single-row patch, cheap against a live reader in a way a DB-row cache
    /// is not: cloning converted rows allocates the `Vec` and bumps refcounts, where
    /// cloning DB rows duplicates every string in the list.
    fn patch(&self, id: i64, edit: impl FnOnce(&mut UiTrackListRow)) {
        let mut guard = self.data.lock();
        let Some(at) = guard.sort.iter().position(|k| k.id == id) else {
            return;
        };
        let data = Arc::make_mut(&mut *guard);
        edit(&mut Arc::make_mut(&mut data.rows)[at]);
    }
}

/// Convert DB rows into the three aligned vectors the cache holds.
///
/// Takes the `Vec` **by value and consumes it**: each DB row's `String`s are freed as its
/// converted row is built, so the peak never holds two full copies of the library's text.
/// Collecting from a borrow would, and that peak is the app's largest allocation.
fn convert(
    rows: Vec<RsTrackListRow>,
) -> (Vec<UiTrackListRow>, Vec<RowSearchKey>, Vec<TrackSortKey>) {
    let n = rows.len();
    let mut display = Vec::with_capacity(n);
    let mut search = Vec::with_capacity(n);
    let mut sort = Vec::with_capacity(n);

    for row in rows {
        search.push(RowSearchKey::from_row(&row));
        sort.push(TrackSortKey {
            id: row.id,
            disc: row.disc(),
            sort_key: row.sort_key.as_deref().unwrap_or("").into(),
        });
        display.push(to_slint_track_list_row(&row));
    }
    (display, search, sort)
}

/// Zip the two parallel vectors into borrowing views for the shared comparator, so the
/// cache runs the app's one sort rather than a second copy of its arms.
fn compute_order(
    rows: &[UiTrackListRow],
    sort: &[TrackSortKey],
    field: &str,
    dir: &str,
) -> Vec<usize> {
    let pairs: Vec<SortRow<'_>> =
        rows.iter().zip(sort).map(|(row, key)| SortRow { row, key }).collect();
    track_sort::compute_track_order(&pairs, field, dir)
}

#[cfg(test)]
#[path = "tests/track_list_cache_tests.rs"]
mod tests;
