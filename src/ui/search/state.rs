//! Internal data structures for the Search view. Mirrors
//! `src/ui/favorites/state.rs` in shape — concentrated here so the public
//! [`crate::ui::search::SearchUi`] surface stays small.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use parking_lot::Mutex;

use crate::library::search::SearchResults;
use crate::services::settings::{SortDir, ViewSort};

/// Per-fetch cached snapshots. Every successful FTS+LIKE round-trip
/// lands here so the compact↔full track toggle, the in-memory sort, and
/// re-stamp paths can recover the underlying Rust data without
/// re-querying the DB.
pub(crate) struct SearchUiState {
    /// The query that produced `last_results`. Mirrors `Search.query` at
    /// commit time (NOT at every keystroke) so a sort handler that needs
    /// the active query string never observes a half-typed value.
    pub last_query: Mutex<String>,
    /// Most-recent successful `library::search::search_all` result. Held
    /// so the "Show all N" toggle and the in-memory sort handler can
    /// re-derive the visible model without touching `SQLite`. Cleared on
    /// section leave + on every new commit before the SQL fetch lands.
    pub last_results: Mutex<Option<SearchResults>>,
    /// Mirror of the persisted JSON history file. Rust pushes this into
    /// `Search.recent-rows` after every add/remove/clear; callers read
    /// it for off-UI-thread access (sort handler shouldn't reach into
    /// the Slint global).
    pub recent: Mutex<Vec<String>>,
    /// Active sort over the Songs section — backend returns FTS rank
    /// order by default, but the user can pick any of the standard
    /// track-list sort fields after the fact. Persisted to
    /// `views.json`'s `view_sort["search"]`.
    pub sort: Mutex<ViewSort>,
    /// Set of `TrackListRow.id`s currently `selected: true` on the
    /// Slint model. Same diff-then-write pattern Favorites uses.
    pub applied_selection: Mutex<HashSet<i32>>,
}

impl SearchUiState {
    pub(super) fn new() -> Self {
        Self {
            last_query: Mutex::new(String::new()),
            last_results: Mutex::new(None),
            recent: Mutex::new(Vec::new()),
            // `"rank"` is the synthetic field the Songs section starts
            // with — the FTS5 query already orders by `rank` so the
            // first apply is a no-op. Switching to any column-based
            // field re-sorts in-memory via `track_sort::sort_track_rows_by`.
            sort: Mutex::new(ViewSort {
                field: "rank".to_owned(),
                dir: SortDir::Asc,
            }),
            applied_selection: Mutex::new(HashSet::new()),
        }
    }
}

/// Albums-strip tile size (px). The strip renders 160 px square cards, and
/// `FemtoVG` minifies with plain bilinear (no mipmaps), so staying near the
/// on-screen size keeps `image-fit: cover` clean without the album grid's
/// 448 px tier. Sized against these cards rather than against another view's
/// tier: the grids draw flex-filled cards that keep growing with the window
/// and take that tier for it, so a strip following them would decode a tile
/// several times larger than any card it can draw.
pub(super) const ALBUM_STRIP_THUMB_SIZE: u32 = 180;

/// Artists-strip tile size (px). Slightly larger than the albums tier
/// because the circular avatar reads softer when downscaled less
/// aggressively.
pub(super) const ARTIST_STRIP_THUMB_SIZE: u32 = 200;

/// LRU capacity per strip — `search_all` clamps album + artist results
/// to 20 each, so a cap of 24 covers a fresh swap without thrashing
/// (the next-set decode of a 20-row strip won't evict the first row
/// before it's painted).
pub(super) const STRIP_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(24) {
    Some(n) => n,
    None => panic!("STRIP_THUMB_CAP > 0"),
};

/// Number of Songs rows shown when `show-all-tracks` is off. The
/// toggle expands to whatever `search_all` returned — its own `LIMIT`
/// is the only cap, so don't restate that number here; clicking "Show
/// less" or starting a new query collapses back to this one.
pub(super) const COMPACT_TRACK_LIMIT: usize = 5;
