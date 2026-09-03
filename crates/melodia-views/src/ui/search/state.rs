//! Internal data structures for the Search view. Mirrors `src/ui/favorites/state.rs`
//! in shape, concentrated here so the public [`crate::ui::search::SearchUi`] surface
//! stays small.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use parking_lot::Mutex;

use crate::library::search::SearchResults;
use crate::services::settings::{SortDir, ViewSort};

/// Per-fetch cached snapshots. Every successful FTS+LIKE round-trip lands here so the
/// compact↔full track toggle, the in-memory sort and the re-stamp paths can recover
/// the underlying Rust data without re-querying.
pub(crate) struct SearchUiState {
    /// The query that produced `last_results`. Mirrors `Search.query` at *commit*
    /// time, not at every keystroke, so a sort handler never observes a half-typed
    /// value.
    pub last_query: Mutex<String>,
    /// Most-recent successful `library::search::search_all`, held so the "Show all N"
    /// toggle and the sort handler re-derive the visible model without touching
    /// `SQLite`. Cleared on section leave and on every new commit before its fetch
    /// lands.
    pub last_results: Mutex<Option<SearchResults>>,
    /// Mirror of the persisted history file, pushed into `Search.recent-rows` after
    /// every add / remove / clear — the sort handler runs off the UI thread and
    /// shouldn't reach into the Slint global.
    pub recent: Mutex<Vec<String>>,
    /// Active sort over the Songs section. The backend returns FTS rank order, and the
    /// user can pick any standard track-list field after the fact; persisted to
    /// `views.json`'s `view_sort["search"]`.
    pub sort: Mutex<ViewSort>,
    /// `TrackListRow.id`s currently `selected: true` on the Slint model. The same
    /// diff-then-write Favorites uses.
    pub applied_selection: Mutex<HashSet<i32>>,
}

impl SearchUiState {
    pub(super) fn new() -> Self {
        Self {
            last_query: Mutex::new(String::new()),
            last_results: Mutex::new(None),
            recent: Mutex::new(Vec::new()),
            // `"rank"` is synthetic: the FTS5 query already orders by it, so the first
            // apply is a no-op. Any column field re-sorts in memory through
            // `track_sort::sort_track_rows_by`.
            sort: Mutex::new(ViewSort {
                field: "rank".to_owned(),
                dir: SortDir::Asc,
            }),
            applied_selection: Mutex::new(HashSet::new()),
        }
    }
}

/// Albums-strip tile size (px). The strip renders 160 px square cards and `FemtoVG`
/// minifies with plain bilinear, so staying near the on-screen size keeps
/// `image-fit: cover` clean. Sized against *these* cards rather than another view's
/// tier: the grids draw flex-filled cards that grow with the window, so a strip
/// following them would decode tiles several times larger than any card it can draw.
pub(super) const ALBUM_STRIP_THUMB_SIZE: u32 = 180;

/// Artists-strip tile size (px). Larger than the albums tier because the circular
/// avatar reads softer when downscaled less aggressively.
pub(super) const ARTIST_STRIP_THUMB_SIZE: u32 = 200;

/// LRU capacity per strip. `search_all` clamps album and artist results to 20 each, so
/// 24 covers a fresh swap without the next set's decode evicting the first row before
/// it is painted.
pub(super) const STRIP_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(24) {
    Some(n) => n,
    None => panic!("STRIP_THUMB_CAP > 0"),
};

/// Songs rows shown when `show-all-tracks` is off. The toggle expands to whatever
/// `search_all` returned — its own `LIMIT` is the only cap, so don't restate that
/// number here.
pub(super) const COMPACT_TRACK_LIMIT: usize = 5;
