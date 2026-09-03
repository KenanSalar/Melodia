//! Internal data structures used by the Genres grid and Genre Detail submodules. Mirror of
//! `src/ui/albums/state.rs` minus everything related to cover thumbnails, genres having no
//! artwork: no LRU to size and no `(cover, blur)` pair to pass between threads.

use std::collections::HashSet;

use parking_lot::Mutex;
use std::sync::Arc;

use crate::entities::genre::GenreStats;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::ui::row_match::Needle;

/// A genre's pre-lowercased name, computed once per `fetch_grid` so the name sort allocates
/// nothing. Positionally aligned with [`GridData::genres`]. One field, unlike `AlbumSortKey` —
/// genres have no secondary text dimension to sort on.
pub(super) struct GenreSortKey {
    pub name_lc: String,
}

/// The grid's canonical data: the genre list plus its pre-lowercased sort keys, kept together
/// behind one `Arc` so a rebuild is a single refcount bump and the two halves can't drift.
pub(super) struct GridData {
    pub genres: Vec<GenreStats>,
    pub keys: Vec<GenreSortKey>,
}

impl GridData {
    /// Build the keys alongside the genres. Runs on a tokio worker, never on the UI thread.
    pub(super) fn new(genres: Vec<GenreStats>) -> Self {
        let keys = genres
            .iter()
            .map(|g| GenreSortKey {
                name_lc: g.name.to_lowercase(),
            })
            .collect();
        Self { genres, keys }
    }
}

/// Memoized filter + sort result — the genre indices into [`GridData::genres`] in display order,
/// plus the `(filter, sort_field, sort_dir)` that produced them. A pure `columns-changed` re-chunk
/// reuses `indices`; a filter or sort change recomputes. Cleared whenever `fetch_grid` replaces
/// the grid data.
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

/// Grid-side state — the canonical genre data the card grid derives from.
pub(super) struct GenreGridState {
    /// Canonical genre data — raw from `genre_stats`, itself name-sorted, plus pre-lowercased
    /// keys. Grid rebuilds derive from this without a DB hit; behind `Mutex<Arc<…>>` so a rebuild
    /// takes a refcount bump instead of a deep clone.
    pub data: Mutex<Arc<GridData>>,
    /// Last filter+sort result, so a `columns-changed` rebuild only needs to re-chunk. `None`
    /// until the first rebuild and after every `fetch_grid`.
    pub index_cache: Mutex<Option<GridIndexCache>>,
}

/// Detail-side state — the currently-open genre's cached track list.
pub(super) struct GenreDetailState {
    /// Cached detail track rows — the **displayed**, filter-applied subset, kept in lockstep with
    /// the Slint `tracks` model so the generic selection/sort logic stays valid. Mirrors
    /// `AlbumDetailState::tracks`.
    pub tracks: Mutex<Vec<RsTrackListRow>>,
    /// Canonical full track set for this genre, in display-sort order. `apply_filtered_detail`
    /// re-derives `tracks` by walking this through the current filter, so the two are equal
    /// whenever no filter is active.
    pub all_tracks: Mutex<Vec<RsTrackListRow>>,
    /// Genre id currently shown in the detail view (`-1` = none), so the library-changed
    /// subscriber can decide whether to refresh it.
    pub genre_id: Mutex<i64>,
    /// The selection set currently *stamped* onto the Slint row model. `apply_selection_to_rows`
    /// diffs the desired selection against this and re-writes only the rows whose membership
    /// flipped. Reset to empty whenever the row model is rebuilt fresh.
    pub applied_selection: Mutex<HashSet<i32>>,
    /// Live filter needle, folded by `set_filter` through
    /// `ui::row_match::fold_needle` — never a bare `to_lowercase`, which
    /// would still build and silently drop accent parity on this one view.
    /// Mirrors `GenreDetail.filter`. Lets
    /// the re-fetch path (`refresh_detail`) re-apply the filter to fresh
    /// data without round-tripping the UI thread for the property read.
    /// Cleared on fresh-open. Mirrors `ArtistDetailState::filter`.
    pub filter: Mutex<Needle>,
}
