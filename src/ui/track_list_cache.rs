//! The retained row set behind a filterable, sortable track list.
//!
//! Two views keep an entire track list resident so a keystroke or a header
//! click costs no DB round trip — My Library's Songs tab, which holds the
//! whole library, and Favorites' Songs tab. Both used to retain the *DB*
//! rows and rebuild every visible `TrackListRow` from them on each rebuild,
//! which put the display strings on the heap twice: once as the `String`s of
//! a `crate::entities::track::TrackListRow` and again as the `SharedString`s
//! of the Slint model built out of it. On a large library that second copy
//! is the larger half of a list view's footprint.
//!
//! So the cache holds **already-converted** rows. `slint::SharedString` is
//! refcounted (`SharedVector<u8>` behind a `derive(Clone)`), so handing a
//! cached row to the model is a pointer copy and a handful of atomic
//! increments, where `SharedString::from(&str)` allocates and copies. Three
//! things fall out of that and none of them is incidental:
//!
//! * the display text exists once rather than twice, and the transient
//!   `Vec` a rebuild builds costs a struct per row instead of a string set;
//! * a filter keystroke stops allocating per visible row — including
//!   `display_duration`, whose `format!` now runs once per fetch;
//! * the single-row favourite / rating patches stop being able to deep-clone
//!   a library's worth of `String`s. They are copy-on-write against a live
//!   reader (a rebuild holding a snapshot), and cloning a `Vec` of converted
//!   rows allocates once instead of ten times per row.
//!
//! What the DB row still answers and a converted one cannot is the sort:
//! `disc_number` never reaches the UI, and `sort_key` — the natural-order
//! key, which is both the default sort's primary and every arm's
//! tie-breaker — is not a displayed column. Those two plus the untruncated
//! `i64` id are [`TrackSortKey`], and [`SortRow`] is the pair viewed as one
//! thing so `track_sort` can compare it. Everything else the arms read is
//! already on the display row.
//!
//! **Alignment is the invariant, and one lock is how it is kept.** `rows`,
//! `search` and `sort` are positionally aligned and `order` is a permutation
//! of their indices, so all four live in one [`CacheData`] behind one
//! `Mutex<Arc<…>>`: a reader takes a single lock, clones one `Arc` and is
//! guaranteed a consistent set. The three-lock version this replaced could
//! be swapped between locks by a concurrent fetch and needed a `.get(i)` on
//! every index to stay panic-safe. The two key vectors are `Arc`s *inside*
//! `CacheData` for the copy-on-write reason above — they are never patched,
//! so cloning the struct bumps their refcounts instead of duplicating a
//! `Box<str>` per row.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::ui::row_match::{self, Needle};
use crate::ui::track_sort::{self, TrackSortFields};
use crate::ui::tracks::{finish_track_list_row, prepare_track_list_row};
use crate::ui::util::len_as_i32;
use crate::TrackListRow as UiTrackListRow;

/// Pre-folded text columns for filtering, one per row.
///
/// Storage is a single packed `Box<str>` of [`row_match::search_fields`]
/// joined by `\0` — one heap allocation per row instead of one `String`
/// header per column. `\0` is a safe separator because needles come from
/// text input (no NUL) and `push_folded` maps away the rare field value
/// carrying one.
///
/// `year` stays an integer beside the packed text rather than being
/// rendered into it, so this and every `track_matches` surface run the
/// *same* [`Needle::matches_year`] rule instead of two spellings of it.
///
/// `Clone` is for [`TrackListCache::remove`] alone — the only operation that
/// changes the set's length, and so the only one that can't leave a reader's
/// snapshot pointing at the vector it is shortening.
#[derive(Clone)]
pub struct RowSearchKey {
    packed: Box<str>,
    year: Option<i32>,
}

impl RowSearchKey {
    pub fn from_row(r: &RsTrackListRow) -> Self {
        let fields = row_match::search_fields(r);
        let text_len: usize = fields.iter().map(|f| f.len()).sum();
        // One separator between each pair, so one fewer than there are fields.
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

    /// A plain `str::contains` on the packed text, because both sides are
    /// already folded — this is the one matcher that doesn't go through
    /// [`Needle::contains`], and `Needle::as_str` exists for it.
    pub fn matches(&self, needle: &Needle) -> bool {
        self.packed.contains(needle.as_str()) || needle.matches_year(self.year)
    }
}

/// What a sort needs and a converted display row cannot answer.
///
/// Three fields, and each is here for its own reason: `id` because the UI
/// row's is clamped to `i32` while the play and queue paths take `i64`,
/// `sort_key` because it is not a displayed column, and `disc` because the
/// disc number reaches no cell. `disc` is stored already flattened onto the
/// `TrackSortFields` sentinel; `sort_key` is stored raw, the comparator
/// owning the case fold exactly as it does for a DB row.
///
/// `Clone` for [`TrackListCache::remove`]'s sake, as [`RowSearchKey`].
#[derive(Clone)]
struct TrackSortKey {
    id: i64,
    sort_key: Box<str>,
    disc: i32,
}

/// A cached row and its sort key viewed as one comparable thing.
///
/// Exists because [`track_sort::sort_track_rows_by`] projects each element
/// to a single `&impl TrackSortFields`, and here the fields are split across
/// two parallel `Vec`s.
struct SortRow<'a> {
    row: &'a UiTrackListRow,
    key: &'a TrackSortKey,
}

/// Every conversion here undoes one [`prepare_track_list_row`] already made,
/// so the order this produces is the order the DB rows produce — which
/// `the_cache_sorts_exactly_as_the_db_rows_do` holds it to.
///
/// The three text fields were `unwrap_or("")`, which is where a `NULL`
/// already sorted, so they pass straight through. `track_number` was
/// `unwrap_or(0)` and its arm reads `0` as "unnumbered", so the sentinel has
/// to be rebuilt or an untracked row sorts first instead of last. `year` was
/// `unwrap_or(0)` too and is the one that needs nothing: no reachable year is
/// negative, so a `0` and a `None` sit in the same place against every other
/// row. That leaves the two indistinguishable here where a DB row still
/// separates them — see `the_cache_conflates_a_zero_year_with_a_missing_one`,
/// which records it precisely because it is unobservable on screen.
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

/// The four aligned vectors, swapped as one so a reader can never see a
/// half-updated set. See the module doc for why the inner two are `Arc`s.
///
/// Cloning is the copy-on-write step behind every single-row patch: one
/// `Vec` allocation plus a refcount bump per `SharedString` and per inner
/// `Arc`, and no string data copied.
#[derive(Clone)]
pub struct CacheData {
    /// Display rows in fetch order — never reordered, only patched in place
    /// by the single-row favourite / rating helpers.
    rows: Vec<UiTrackListRow>,
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
            rows: Vec::new(),
            search: Arc::new(Vec::new()),
            sort: Arc::new(Vec::new()),
            order: Arc::new(Vec::new()),
        }
    }

    /// Rows passing `needle`, in display order, ready for the Slint model.
    ///
    /// Each row is a `clone` of a cached one — a struct copy plus one atomic
    /// increment per `SharedString`, no allocation. This is what runs per
    /// throttled keystroke.
    pub fn visible(&self, needle: &Needle) -> Vec<UiTrackListRow> {
        self.walk(needle).map(|(_, row)| row.clone()).collect()
    }

    /// Ids of the rows passing `needle`, in display order — what a row
    /// activation hands to `player_play_tracks` so the queue becomes exactly
    /// the list the user is looking at.
    pub fn ids_filtered(&self, needle: &Needle) -> Vec<i64> {
        self.walk(needle).map(|(i, _)| self.sort[i].id).collect()
    }

    /// Unique artwork paths in **display** order, capped — so that on a
    /// library with more unique covers than the thumbnail cache holds, the
    /// entries surviving the cap are the ones the user sees first.
    pub fn artwork_paths(&self, cap: usize) -> Vec<PathBuf> {
        crate::ui::grid_prewarm::unique_artwork_paths(
            self.order
                .iter()
                .map(|&i| Some(self.rows[i].artwork_path.as_str())),
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

    /// The one walk `visible` and `ids_filtered` share: display order,
    /// filtered, yielding each surviving row beside its cache index.
    ///
    /// Indexing rather than `.get()` is safe here in a way it was not before
    /// the four vectors shared a lock: `order` is built from these very rows
    /// and is replaced with them, so no concurrent fetch can leave an index
    /// pointing past the set it indexes.
    fn walk<'a>(&'a self, needle: &'a Needle) -> impl Iterator<Item = (usize, &'a UiTrackListRow)> {
        self.order
            .iter()
            .map(|&i| (i, &self.rows[i]))
            .filter(move |(i, _)| needle.is_empty() || self.search[*i].matches(needle))
    }
}

/// A filterable, sortable track list held in memory.
///
/// Cheap to snapshot and safe to read from any thread; see the module doc
/// for the residency argument and the alignment invariant.
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

    /// A consistent view of all four vectors, for a rebuild that must not
    /// hold the lock while it walks.
    pub fn snapshot(&self) -> Arc<CacheData> {
        self.data.lock().clone()
    }

    /// Convert `rows` into the cache and compute the display order for
    /// `field` / `dir`.
    pub fn store(&self, rows: Vec<RsTrackListRow>, field: &str, dir: &str) {
        let (display, search, sort) = convert(rows);
        let order = compute_order(&display, &sort, field, dir);
        self.install(display, search, sort, order);
    }

    /// [`Self::store`] for a caller that has already computed the display
    /// permutation over the same rows.
    ///
    /// Favorites needs it: its cover prewarm walks the list in display order
    /// and has to run *before* the section guards allow the store, so the
    /// permutation exists a moment early and re-deriving it here would sort
    /// the set twice per fetch. Handing it over is only sound because the
    /// permutation the shared comparator produces over `TrackListRow`s is the
    /// one it produces over this cache's converted rows —
    /// `the_cache_sorts_exactly_as_the_db_rows_do` is what holds that.
    pub fn store_in_order(&self, rows: Vec<RsTrackListRow>, order: Vec<usize>) {
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
            rows,
            search: Arc::new(search),
            sort: Arc::new(sort),
            order: Arc::new(order),
        });
    }

    /// Recompute only the display permutation — a header click costs no DB
    /// round trip and no key rebuild, the `(row, key)` set being unchanged by
    /// a re-order.
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

    /// Patch `is_favorite` on one row, so a toggle doesn't re-fetch the list
    /// (which would lose scroll position and flash).
    pub fn set_favorite(&self, id: i64, fav: bool) {
        self.patch(id, |row| row.is_favorite = fav);
    }

    /// Patch `rating` on one row — the star-rating analogue of
    /// [`Self::set_favorite`].
    pub fn set_rating(&self, id: i64, rating: i32) {
        self.patch(id, |row| row.rating = rating);
    }

    /// Drop one row from all four vectors, keeping them aligned.
    ///
    /// Favorites needs this: unfavouriting a track removes it from the set
    /// the list is defined by (`is_favorite = TRUE`), so patching the row
    /// would leave it on screen. `order` is rebuilt rather than recomputed —
    /// dropping the removed slot and shifting the indices past it preserves
    /// the current sort exactly, where a re-sort would need the field and
    /// direction the caller no longer has.
    pub fn remove(&self, id: i64) {
        let mut guard = self.data.lock();
        let Some(at) = guard.sort.iter().position(|k| k.id == id) else {
            return;
        };
        let data = Arc::make_mut(&mut *guard);
        data.rows.remove(at);
        Arc::make_mut(&mut data.search).remove(at);
        Arc::make_mut(&mut data.sort).remove(at);
        let order = Arc::make_mut(&mut data.order);
        order.retain(|&i| i != at);
        for i in order.iter_mut().filter(|i| **i > at) {
            *i -= 1;
        }
    }

    /// Copy-on-write single-row patch. Cheap against a live reader in a way
    /// the DB-row cache this replaced was not — cloning converted rows
    /// allocates the `Vec` and bumps refcounts, where cloning DB rows
    /// duplicated every string in the list.
    fn patch(&self, id: i64, edit: impl FnOnce(&mut UiTrackListRow)) {
        let mut guard = self.data.lock();
        let Some(at) = guard.sort.iter().position(|k| k.id == id) else {
            return;
        };
        edit(&mut Arc::make_mut(&mut *guard).rows[at]);
    }
}

/// Convert DB rows into the three aligned vectors the cache holds.
///
/// Takes the `Vec` **by value and consumes it**: each DB row's ten `String`s
/// are freed as its converted row is built, so the peak never holds two full
/// copies of the library's text. Collecting from a borrow would, and that
/// peak is the largest single allocation the app makes.
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
        display.push(finish_track_list_row(prepare_track_list_row(&row)));
    }
    (display, search, sort)
}

/// Zip the two parallel vectors into borrowing views and hand them to the
/// shared comparator, so the cache runs the app's one sort rather than a
/// second copy of its arms.
fn compute_order(
    rows: &[UiTrackListRow],
    sort: &[TrackSortKey],
    field: &str,
    dir: &str,
) -> Vec<usize> {
    let pairs: Vec<SortRow<'_>> = rows
        .iter()
        .zip(sort)
        .map(|(row, key)| SortRow { row, key })
        .collect();
    track_sort::compute_track_order(&pairs, field, dir)
}

#[cfg(test)]
#[path = "tests/track_list_cache_tests.rs"]
mod tests;
