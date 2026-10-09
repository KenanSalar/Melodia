//! The Albums grid's data structures and constants. The detail's are
//! [`crate::ui::track_detail::DetailCache`].

use std::sync::Arc;

use parking_lot::Mutex;

use melodia_core::entities::album::AlbumStats;

/// An album's pre-lowercased name + artist, computed once per `fetch_grid`
/// so the name / artist sorts allocate nothing. Positionally aligned with
/// [`GridData::albums`]. The filter doesn't read it — it walks the raw
/// fields through `ui::row_match`, which has to fold accents and so can't
/// take a plain lowercased key.
pub(super) struct AlbumSortKey {
    pub name_lc: String,
    pub artist_lc: String,
}

/// The grid's canonical data: the album list plus its pre-lowercased
/// sort keys, kept together behind one `Arc` so a rebuild is a
/// single refcount bump (and the two halves can never drift out of sync).
pub(super) struct GridData {
    pub albums: Vec<AlbumStats>,
    pub keys: Vec<AlbumSortKey>,
}

impl GridData {
    /// Build the keys alongside the albums. Runs on a tokio worker (inside
    /// `fetch_grid`), never on the UI thread.
    pub(super) fn new(albums: Vec<AlbumStats>) -> Self {
        let keys = albums
            .iter()
            .map(|a| AlbumSortKey {
                name_lc: a.name.to_lowercase(),
                artist_lc: a.artist_name.to_lowercase(),
            })
            .collect();
        Self { albums, keys }
    }
}

/// Memoized filter + sort result — the album indices into
/// [`GridData::albums`] in display order, plus the `(filter, sort_field,
/// sort_dir)` that produced them. A pure `columns-changed` re-chunk reuses
/// `indices` without re-filtering / re-sorting; a filter or sort change
/// recomputes. Cleared whenever `fetch_grid` replaces the grid data.
pub(super) struct GridIndexCache {
    pub filter: String,
    pub sort_field: String,
    pub sort_dir: String,
    pub indices: Vec<usize>,
}

impl GridIndexCache {
    /// Whether this cache entry was produced by the given filter/sort.
    pub(super) fn matches(&self, filter: &str, sort_field: &str, sort_dir: &str) -> bool {
        self.filter == filter && self.sort_field == sort_field && self.sort_dir == sort_dir
    }
}

/// Grid-side state — the canonical album data the card grid derives from.
pub(super) struct AlbumGridState {
    /// Canonical album data — raw from `album_stats` (itself name-sorted)
    /// plus pre-lowercased keys. Grid rebuilds (filter / sort / re-chunk)
    /// derive from this without a DB hit. Behind `Mutex<Arc<…>>` so a
    /// rebuild takes a cheap refcount bump instead of deep-cloning.
    pub data: Mutex<Arc<GridData>>,
    /// Last filter+sort result, so a `columns-changed` rebuild only needs
    /// to re-chunk. `None` until the first rebuild and after every
    /// `fetch_grid`.
    pub index_cache: Mutex<Option<GridIndexCache>>,
}

/// How many leading (name-sorted) albums' covers `fetch_grid` prewarms
/// before the grid first paints. Covers roughly the first screenful at any
/// reasonable column count; everything past it decodes lazily on
/// scroll-in via `request-cover`. Kept ≤ the `ui::grid_prewarm::cover_cap`
/// floor (32) so the prewarm can't thrash the grid-tier LRU it fills —
/// a grid-tier buffer is hundreds of KB, so this stays deliberately small.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
