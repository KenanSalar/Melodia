//! Internal data structures + constants used by the Albums grid and
//! Album Detail submodules.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::entities::album::AlbumStats;
use crate::entities::track::TrackListRow as RsTrackListRow;

/// An album's pre-lowercased name + artist, computed once per `fetch_grid`
/// so the per-keystroke filter walk and the name / artist sorts allocate
/// nothing. Positionally aligned with [`GridData::albums`].
pub(super) struct AlbumSearchKey {
    pub name_lc: String,
    pub artist_lc: String,
}

/// The grid's canonical data: the album list plus its pre-lowercased
/// search / sort keys, kept together behind one `Arc` so a rebuild is a
/// single refcount bump (and the two halves can never drift out of sync).
pub(super) struct GridData {
    pub albums: Vec<AlbumStats>,
    pub keys: Vec<AlbumSearchKey>,
}

impl GridData {
    /// Build the keys alongside the albums. Runs on a tokio worker (inside
    /// `fetch_grid`), never on the UI thread.
    pub(super) fn new(albums: Vec<AlbumStats>) -> Self {
        let keys = albums
            .iter()
            .map(|a| AlbumSearchKey {
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

/// Detail-side state — the currently-open album's cached track list.
pub(super) struct AlbumDetailState {
    /// Cached detail track rows — the **displayed** (filter-applied)
    /// subset, kept in lockstep with the Slint `tracks` model so the
    /// generic selection/sort logic (which maps id ↔ row-index through
    /// this cache) stays valid. `play-row` / `select-row` / `play-album`
    /// / the in-memory re-sort read this without round-tripping the
    /// Slint model — mirrors `BrowseUi::last_files`.
    pub tracks: Mutex<Vec<RsTrackListRow>>,
    /// Canonical full track set for this album, in display-sort order.
    /// `tracks` holds only the displayed subset; `apply_filtered_detail`
    /// re-derives `tracks` by walking this through the current filter.
    /// Equal to `tracks` whenever no filter is active.
    pub all_tracks: Mutex<Vec<RsTrackListRow>>,
    /// Album id currently shown in the detail view (`-1` = none). Lets the
    /// library-changed subscriber decide whether to refresh the detail.
    pub album_id: Mutex<i64>,
    /// The selection set currently *stamped* onto the Slint row model.
    /// `apply_selection_to_rows` diffs the desired selection against this
    /// and only re-writes the rows whose membership flipped — O(changed),
    /// not O(rows). Reset to empty whenever the row model is rebuilt fresh.
    pub applied_selection: Mutex<HashSet<i32>>,
    /// Live lowercased filter needle, mirroring `AlbumDetail.filter`. Lets
    /// the re-fetch path (`refresh_detail`) re-apply the filter to fresh
    /// data without round-tripping the UI thread for the property read.
    /// Cleared on fresh-open. Mirrors `ArtistDetailState::filter`.
    pub filter: Mutex<String>,
}

/// Square decode size (px) for the Albums **grid card** tiles. Bigger than
/// the detail header tier because the flex-filled grid cards get large on a
/// wide panel (well past 280 px), and a tile upscaled past its decode size
/// visibly softens. The knob to retune grid sharpness.
pub(super) const GRID_COVER_SIZE: u32 = 448;

/// Fallback LRU capacity for the **grid** cover cache, used at construction
/// and whenever the display can't be queried. `tune_cache_for_display`
/// replaces it at startup with a resolution-derived cap. Must stay ≥
/// `GRID_PREWARM_AHEAD` or the prewarm would thrash its own LRU.
pub(super) const DEFAULT_GRID_COVER_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("DEFAULT_GRID_COVER_CAP > 0"),
};

/// How many leading (name-sorted) albums' covers `fetch_grid` prewarms
/// before the grid first paints. Covers roughly the first screenful at any
/// reasonable column count; everything past it decodes lazily on
/// scroll-in via `request-cover`. Kept ≤ the `compute_album_cover_cap`
/// floor (32) so the prewarm can't thrash the grid-tier LRU it fills —
/// each 448 px buffer is ~600 KB, so this stays deliberately small.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
