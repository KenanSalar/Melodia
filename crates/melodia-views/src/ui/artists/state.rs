//! Internal data structures + constants used by the Artists grid and
//! Artist Detail submodules. Mirrors `src/ui/albums/state.rs`.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ui::row_match::Needle;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;

/// An artist's pre-lowercased `name`, computed once per `fetch_grid` so
/// the name sort allocates nothing. Positionally aligned with
/// [`GridData::artists`]. The filter doesn't read it — it walks the raw
/// fields through `ui::row_match`, which has to fold accents and so
/// can't take a plain lowercased key.
pub(super) struct ArtistSortKey {
    pub name_lc: String,
}

/// The grid's canonical data: the artist list plus its pre-lowercased
/// sort keys, kept together behind one `Arc` so a rebuild is a
/// single refcount bump.
pub(super) struct GridData {
    pub artists: Vec<ArtistStats>,
    pub keys: Vec<ArtistSortKey>,
}

impl GridData {
    pub(super) fn new(artists: Vec<ArtistStats>) -> Self {
        let keys = artists
            .iter()
            .map(|a| ArtistSortKey {
                name_lc: a.name.to_lowercase(),
            })
            .collect();
        Self { artists, keys }
    }
}

/// Memoized filter + sort result — the artist indices into
/// [`GridData::artists`] in display order, plus the `(filter, sort_field,
/// sort_dir)` that produced them. Cleared whenever `fetch_grid` replaces
/// the grid data.
pub(super) struct GridIndexCache {
    pub filter: String,
    pub sort_field: String,
    pub sort_dir: String,
    pub indices: Vec<usize>,
}

impl GridIndexCache {
    pub(super) fn matches(&self, filter: &str, sort_field: &str, sort_dir: &str) -> bool {
        self.filter == filter && self.sort_field == sort_field && self.sort_dir == sort_dir
    }
}

/// Grid-side state — the canonical artist data the card grid derives from.
pub(super) struct ArtistGridState {
    pub data: Mutex<Arc<GridData>>,
    pub index_cache: Mutex<Option<GridIndexCache>>,
}

/// Detail-side state — the currently-open artist's cached track list +
/// album sub-section + filter needle + applied-selection shadow.
///
/// `tracks` and `albums` are the unfiltered canonical lists; the per-
/// keystroke filter walk runs over these in memory (no DB round-trip)
/// and re-stamps the Slint `tracks` / `albums` models with the filtered
/// subsets. `filter` is the live needle, folded by `set_filter` through
/// `ui::row_match::fold_needle` (never a bare `to_lowercase`, which would
/// still build and silently drop accent parity on this one view) —
/// mirroring the Slint `ArtistDetail.filter` property so a refresh while
/// the user has a filter typed re-applies the filter to fresh data
/// without round-tripping through the UI thread.
pub(super) struct ArtistDetailState {
    /// Displayed (filter-applied) track rows, kept in lockstep with the
    /// Slint `tracks` model so the generic selection/sort logic — which
    /// maps id ↔ row-index through this cache — stays valid.
    pub tracks: Mutex<Vec<RsTrackListRow>>,
    /// Canonical full track set for this artist, in display-sort order.
    /// `apply_filtered_detail` re-derives `tracks` by walking this
    /// through the current filter. Equal to `tracks` when no filter is
    /// active.
    pub all_tracks: Mutex<Vec<RsTrackListRow>>,
    pub albums: Mutex<Vec<AlbumStats>>,
    pub artist_id: Mutex<i64>,
    pub filter: Mutex<Needle>,
    pub applied_selection: Mutex<HashSet<i32>>,
}

/// Fallback LRU capacity. Replaced at startup by
/// `tune_cache_for_display`.
pub(super) const DEFAULT_GRID_COVER_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("DEFAULT_GRID_COVER_CAP > 0"),
};

/// How many leading (name-sorted) artists' covers `fetch_grid` prewarms.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
