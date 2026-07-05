//! Internal data structures for the Recently-Played view. Trimmed sibling
//! of `src/ui/favorites/state.rs` — no hero/mosaic or artist strip, and the
//! track list orders by recency (a fixed fetch order) rather than a DB sort,
//! so the cached set is filtered/re-sorted in memory.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use parking_lot::Mutex;

use crate::entities::track::{MostPlayedFavorite, TrackListRow as RsTrackListRow};
use crate::services::settings::{SortDir, ViewSort};

/// Per-section cached snapshots — every fetch lands here so callbacks can
/// recover the underlying Rust data without round-tripping through Slint.
pub(crate) struct RecentlyPlayedUiState {
    /// The most-recently-played rows in fetch (recency) order, *before* the
    /// in-memory search filter and any column re-sort. Membership is fixed to
    /// this set — the view never re-queries on filter/sort.
    pub tracks_all: Mutex<Vec<RsTrackListRow>>,
    /// Active filter string — cached so the live-refresh subscriber can
    /// re-apply it off the UI thread.
    pub filter: Mutex<String>,
    /// Active sort. Default is the synthetic [`super::RECENCY_SORT`] field
    /// (keep fetch order); a column header click swaps in a real
    /// `TrackListRow` field and the list re-sorts in memory.
    pub sort: Mutex<ViewSort>,
    /// Most Played strip rows in Rust shape — kept so click handlers can
    /// resolve `(id) -> entity` without re-fetching.
    pub most_played: Mutex<Vec<MostPlayedFavorite>>,
    /// Set of `TrackListRow.id`s currently `selected: true` on the Slint
    /// model. Same diff-then-write pattern the other list views use.
    pub applied_selection: Mutex<HashSet<i32>>,
}

impl RecentlyPlayedUiState {
    pub(super) fn new() -> Self {
        Self {
            tracks_all: Mutex::new(Vec::new()),
            filter: Mutex::new(String::new()),
            sort: Mutex::new(ViewSort {
                field: super::RECENCY_SORT.to_owned(),
                dir: SortDir::Desc,
            }),
            most_played: Mutex::new(Vec::new()),
            applied_selection: Mutex::new(HashSet::new()),
        }
    }
}

/// Hero mosaic-tile cache size (px). Each hero tile renders at ~70 px, so
/// 128 px gives a crisp downscale. Mirrors the Favorites mosaic tier.
pub(super) const MOSAIC_THUMB_SIZE: u32 = 128;

/// LRU capacity for the mosaic-tile cache — at most 4 covers live in the
/// mosaic, with a little headroom for the recency set shifting.
pub(super) const MOSAIC_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(n) => n,
    None => panic!("MOSAIC_THUMB_CAP > 0"),
};

/// Most-Played strip tile size (px). The strip renders 160 px square cards, so
/// 180 px keeps `image-fit: cover` mipmaps crisp. Mirrors the Favorites tier.
pub(super) const MOST_PLAYED_THUMB_SIZE: u32 = 180;

/// LRU capacity for the Most Played strip — the query is clamped to 10 results,
/// so 16 covers post-refresh cycling without thrashing.
pub(super) const MOST_PLAYED_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(n) => n,
    None => panic!("MOST_PLAYED_THUMB_CAP > 0"),
};
