//! What [`crate::ui::search::SearchUi`] caches, and the sizes of its tiers.

use std::num::NonZeroUsize;

use parking_lot::Mutex;

use melodia_app::services::settings::ViewSort;
use melodia_core::entities::search::SearchResults;
use melodia_core::entities::track_list::SortDir;

/// The last fetch, kept so the compact/full toggle and the in-memory sort re-derive the view
/// without re-querying.
pub(crate) struct SearchUiState {
    /// The query that produced `last_results`, as committed rather than as typed.
    pub last_query: Mutex<String>,
    pub last_results: Mutex<Option<SearchResults>>,
    /// The Songs list's sort, applied in memory over the rank order the query returns.
    pub sort: Mutex<ViewSort>,
}

impl SearchUiState {
    pub(super) fn new() -> Self {
        Self {
            last_query: Mutex::new(String::new()),
            last_results: Mutex::new(None),
            // `rank` is synthetic, the order the query already returns.
            sort: Mutex::new(ViewSort { field: "rank".to_owned(), dir: SortDir::Asc }),
        }
    }
}

/// Albums-strip tile size (px), near the strip's fixed card size because `FemtoVG` minifies with
/// plain bilinear. Its own rather than a grid tier, the grids' cards growing with the window.
pub(super) const ALBUM_STRIP_THUMB_SIZE: u32 = 180;

/// Artists-strip tile size (px), above the albums' because a circular avatar reads softer when
/// downscaled harder.
pub(super) const ARTIST_STRIP_THUMB_SIZE: u32 = 200;

/// LRU capacity per strip, above the 20 album and 20 artist results `search_all` returns, so a
/// fresh set's decode can't evict its own first row before it paints.
pub(super) const STRIP_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(24) {
    Some(n) => n,
    None => panic!("STRIP_THUMB_CAP > 0"),
};

/// Songs rows shown while `show-all-tracks` is off; "Show all" expands to whatever `search_all`
/// returned.
pub(super) const COMPACT_TRACK_LIMIT: usize = 5;
