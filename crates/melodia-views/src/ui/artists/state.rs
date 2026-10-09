//! Internal data structures + constants used by the Artists grid and
//! Artist Detail submodules. Mirrors `src/ui/albums/state.rs`.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ui::track_detail::DetailCache;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::ArtistStats;

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
        let keys =
            artists.iter().map(|a| ArtistSortKey { name_lc: a.name.to_lowercase() }).collect();
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

/// Detail-side state: the shared track cache plus the Albums strip only this detail has.
pub(super) struct ArtistDetailState {
    pub cache: DetailCache,
    /// The artist's albums, unfiltered: the filter pass narrows the strip from this.
    pub albums: Mutex<Vec<AlbumStats>>,
}

/// How many leading (name-sorted) artists' covers `fetch_grid` prewarms.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
