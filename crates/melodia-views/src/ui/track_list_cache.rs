//! The retained row set behind a filterable, sortable track list.
//!
//! Two views keep an entire track list resident so a keystroke or a header click costs no
//! DB round trip: My Library's Songs tab, holding the whole library, and Favorites'.
//!
//! The cache holds **already-converted** rows rather than DB ones, and the Slint model reads
//! them in place ([`crate::ui::track_list_model`]), so the display text exists once: a filter
//! keystroke allocates one index per surviving row, and only the rows on screen are ever
//! cloned. Once is also across rows: the conversion interns, so an album's tracks hold one
//! artist, album, genre and cover path between them.
//!
//! What a DB row still answers and a converted one cannot is the sort: `disc_number` never
//! reaches the UI and `sort_key` is not a displayed column. Those two plus the untruncated
//! `i64` id are [`TrackSortKey`]; [`SortRow`] is the pair viewed as one thing.
//!
//! **Alignment is the invariant, and one lock is how it is kept.** `rows`, `marks`, `search`
//! and `sort` are positionally aligned and `order` is a permutation of their indices, so all
//! five live in one [`CacheData`] behind one `Mutex<Arc<…>>`. **All five are `Arc`s *inside*
//! it**, so cloning bumps five refcounts rather than duplicating a `Box<str>` per row, and a
//! re-sort, replacing `order` alone, copies none of the others.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use parking_lot::Mutex;

use crate::ui::row_match::{self, Needle};
use crate::ui::track_sort::{self, TrackSortFields};
use crate::ui::tracks::to_slint_track_list_row;
use crate::ui::util::{StringPool, len_as_i32};
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_ui::TrackListRow as UiTrackListRow;

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
            melodia_core::utils::fold::push_folded(&mut buf, field);
        }
        Self { packed: buf.into_boxed_str(), year: r.year }
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

/// The two fields a single-row patch moves, kept beside the rows rather than in them. Every
/// snapshot of one stored set shares them, so a patch reaches the list on screen without
/// copying the set that list holds.
///
/// `Relaxed` throughout: each field stands alone, and the repaint that follows a patch reaches
/// the UI thread through the event loop's queue, which orders the two.
struct RowMarks {
    favorite: AtomicBool,
    rating: AtomicI32,
}

impl RowMarks {
    /// Moves both fields out of `row`, so a stored row never carries a value a patch has since
    /// moved.
    fn take_from(row: &mut UiTrackListRow) -> Self {
        Self {
            favorite: AtomicBool::new(std::mem::take(&mut row.is_favorite)),
            rating: AtomicI32::new(std::mem::take(&mut row.rating)),
        }
    }

    fn stamp(&self, row: &mut UiTrackListRow) {
        row.is_favorite = self.favorite.load(Ordering::Relaxed);
        row.rating = self.rating.load(Ordering::Relaxed);
    }

    fn same_as(&self, other: &Self) -> bool {
        self.favorite.load(Ordering::Relaxed) == other.favorite.load(Ordering::Relaxed)
            && self.rating.load(Ordering::Relaxed) == other.rating.load(Ordering::Relaxed)
    }
}

/// For [`TrackListCache::remove`], whose copy is the one place two snapshots stop sharing marks.
impl Clone for RowMarks {
    fn clone(&self) -> Self {
        Self {
            favorite: AtomicBool::new(self.favorite.load(Ordering::Relaxed)),
            rating: AtomicI32::new(self.rating.load(Ordering::Relaxed)),
        }
    }
}

/// A cached row and its sort key viewed as one comparable thing, because
/// [`track_sort::sort_track_rows_by`] projects each element to a single
/// `&impl TrackSortFields` and here the fields are split across two parallel `Vec`s.
struct SortRow<'a> {
    row: &'a UiTrackListRow,
    key: &'a TrackSortKey,
}

/// Every conversion here undoes one [`crate::ui::tracks::to_slint_track_list_row`] made, so
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

/// The five aligned vectors, swapped as one so a reader can never see a half-updated set.
/// Cloning bumps five refcounts; [`TrackListCache::remove`] is the one operation that then
/// copies what it shares.
#[derive(Clone)]
pub struct CacheData {
    /// Display rows in fetch order, never reordered or patched. `is_favorite` and `rating`
    /// live in `marks` instead, so [`Self::row`] is the only way to read one whole. Behind
    /// its own `Arc` because `Arc::make_mut` on the enclosing [`CacheData`] clones **every**
    /// field it finds shared, so a [`TrackListCache::resort`] landing while a snapshot was
    /// held would duplicate the whole row vector to move a permutation.
    rows: Arc<Vec<UiTrackListRow>>,
    /// Favourite and rating, aligned with `rows` and written through by every patch.
    marks: Arc<Vec<RowMarks>>,
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
            marks: Arc::new(Vec::new()),
            search: Arc::new(Vec::new()),
            sort: Arc::new(Vec::new()),
            order: Arc::new(Vec::new()),
        }
    }

    /// Ids of the rows passing `needle`, in display order — what a row activation hands
    /// to `player_play_tracks` so the queue becomes the list the user is looking at.
    pub fn ids_filtered(&self, needle: &Needle) -> Vec<i64> {
        // A `Filter` adapter reports a `size_hint` lower bound of `0`, so an unfiltered walk
        // reserves exactly, the empty needle being what every cold fetch and library tick
        // takes. A real needle reserves nothing: no cheap guess at the survivor count beats
        // growing, and a library-sized `Vec` holding three ids is the worse wrong.
        let mut out = Vec::with_capacity(if needle.is_empty() { self.rows.len() } else { 0 });
        out.extend(self.walk(needle).map(|i| self.sort[i].id));
        out
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

    /// The cache indices passing `needle`, in display order: the one walk [`VisibleRows`] and
    /// `ids_filtered` share. Indexing rather than `.get()` is safe because the five vectors
    /// share a lock — `order` is built from these very rows and replaced with them, so no
    /// concurrent fetch can leave an index past its set.
    fn walk<'a>(&'a self, needle: &'a Needle) -> impl Iterator<Item = usize> + 'a {
        self.order
            .iter()
            .copied()
            .filter(move |&i| needle.is_empty() || self.search[i].matches(needle))
    }

    /// The row at cache index `i`, with its marks stamped back on.
    fn row(&self, i: usize) -> UiTrackListRow {
        let mut row = self.rows[i].clone();
        self.marks[i].stamp(&mut row);
        row
    }

    /// Whether index `i` here and `j` in `other` would paint the same row, answered without
    /// cloning either.
    fn reads_same(&self, i: usize, other: &Self, j: usize) -> bool {
        self.rows[i] == other.rows[j] && self.marks[i].same_as(&other.marks[j])
    }

    fn position(&self, id: i64) -> Option<usize> {
        self.sort.iter().position(|k| k.id == id)
    }
}

/// One snapshot's rows passing a filter, in display order: what a
/// [`TrackListModel`](crate::ui::track_list_model::TrackListModel) shows. Built on whichever
/// thread holds the snapshot, so a cold fetch's filter walk stays off the event loop and only
/// the finished selection crosses to it.
pub struct VisibleRows {
    data: Arc<CacheData>,
    /// The cache indices passing the filter, or `None` when nothing is filtered out: the
    /// snapshot's own `order` is that answer already, and a second library-sized copy of it
    /// would be the largest allocation a keystroke makes.
    filtered: Option<Vec<usize>>,
}

impl Default for VisibleRows {
    fn default() -> Self {
        Self { data: Arc::new(CacheData::empty()), filtered: None }
    }
}

impl VisibleRows {
    pub fn new(data: Arc<CacheData>, needle: &Needle) -> Self {
        let filtered = (!needle.is_empty()).then(|| data.walk(needle).collect());
        Self { data, filtered }
    }

    pub fn len(&self) -> usize {
        self.indices().len()
    }

    pub fn is_empty(&self) -> bool {
        self.indices().is_empty()
    }

    /// The row at display position `pos`.
    pub fn row(&self, pos: usize) -> Option<UiTrackListRow> {
        self.indices().get(pos).map(|&i| self.data.row(i))
    }

    /// Every id in display order, read in place.
    pub fn ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.indices().iter().map(|&i| self.data.rows[i].id)
    }

    /// Every id the snapshot holds, the ones the filter hides included.
    pub fn held_ids(&self) -> HashSet<i32> {
        self.data.rows.iter().map(|row| row.id).collect()
    }

    /// Whether display position `pos` paints the same row here as in `other`.
    pub fn reads_same_at(&self, other: &Self, pos: usize) -> bool {
        match (self.indices().get(pos), other.indices().get(pos)) {
            (Some(&i), Some(&j)) => self.data.reads_same(i, &other.data, j),
            _ => false,
        }
    }

    fn indices(&self) -> &[usize] {
        self.filtered.as_deref().unwrap_or(&self.data.order)
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
        Self { data: Mutex::new(Arc::new(CacheData::empty())) }
    }

    /// A consistent view of all five vectors, for a rebuild that must not hold the lock
    /// while it walks.
    pub fn snapshot(&self) -> Arc<CacheData> {
        self.data.lock().clone()
    }

    /// Convert `rows` into the cache and compute the display order for `field` / `dir`.
    pub fn store(&self, rows: Vec<RsTrackListRow>, field: &str, dir: &str) {
        self.install(convert(rows, |display, sort| compute_order(display, sort, field, dir)));
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
        self.install(convert(rows, |_, _| order));
    }

    fn install(&self, data: CacheData) {
        *self.data.lock() = Arc::new(data);
    }

    /// Recompute only the display permutation — a header click costs no DB round trip, no
    /// key rebuild and **no row copy either, which is what `rows` being an `Arc` buys**:
    /// `Arc::make_mut` clones every shared field, and the list on screen always holds a
    /// snapshot.
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
        self.mark(id, |marks| marks.favorite.store(fav, Ordering::Relaxed));
    }

    /// Patch `rating` on one row — the star-rating analogue of [`Self::set_favorite`].
    pub fn set_rating(&self, id: i64, rating: i32) {
        self.mark(id, |marks| marks.rating.store(rating, Ordering::Relaxed));
    }

    /// Drop one row from all five vectors, keeping them aligned — Favorites needs it,
    /// unfavouriting removing a track from the set the list is defined by. `order` is
    /// rebuilt rather than recomputed: shifting the indices past the removed slot preserves
    /// the current sort exactly, where a re-sort needs a field and direction the caller
    /// doesn't have.
    ///
    /// Copy-on-write, the list on screen holding the snapshot this shortens: it keeps its
    /// own until the caller shows it the new one.
    pub fn remove(&self, id: i64) {
        let mut guard = self.data.lock();
        let Some(at) = guard.position(id) else {
            return;
        };
        let data = Arc::make_mut(&mut *guard);
        Arc::make_mut(&mut data.rows).remove(at);
        Arc::make_mut(&mut data.marks).remove(at);
        Arc::make_mut(&mut data.search).remove(at);
        Arc::make_mut(&mut data.sort).remove(at);
        let order = Arc::make_mut(&mut data.order);
        order.retain(|&i| i != at);
        for i in order.iter_mut().filter(|i| **i > at) {
            *i -= 1;
        }
    }

    /// Patch one row's marks in place, copying nothing whoever holds a snapshot. The caller
    /// owes the list a repaint of that row.
    fn mark(&self, id: i64, edit: impl FnOnce(&RowMarks)) {
        let data = self.data.lock();
        if let Some(at) = data.position(id) {
            edit(&data.marks[at]);
        }
    }
}

/// Convert DB rows into a cache laid out in the display order `order_of` picks off the
/// converted rows and sort keys.
///
/// Takes the `Vec` **by value and consumes it**: each DB row's `String`s are freed as its
/// converted row is built, so the peak never holds two full copies of the library's text.
/// Collecting from a borrow would, and that peak is the app's largest allocation.
fn convert(
    rows: Vec<RsTrackListRow>,
    order_of: impl FnOnce(&[UiTrackListRow], &[TrackSortKey]) -> Vec<usize>,
) -> CacheData {
    let n = rows.len();
    let mut display = Vec::with_capacity(n);
    let mut marks = Vec::with_capacity(n);
    let mut search = Vec::with_capacity(n);
    let mut sort = Vec::with_capacity(n);
    let mut pool = StringPool::default();

    for row in rows {
        search.push(RowSearchKey::from_row(&row));
        sort.push(TrackSortKey {
            id: row.id,
            disc: row.disc(),
            sort_key: row.sort_key.as_deref().unwrap_or("").into(),
        });
        let mut converted = to_slint_track_list_row(&row, &mut pool);
        marks.push(RowMarks::take_from(&mut converted));
        display.push(converted);
    }
    let order = order_of(&display, &sort);
    CacheData {
        rows: Arc::new(display),
        marks: Arc::new(marks),
        search: Arc::new(search),
        sort: Arc::new(sort),
        order: Arc::new(order),
    }
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
