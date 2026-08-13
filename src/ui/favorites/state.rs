//! Internal data structures used by the Favorites view. Mirrors
//! `src/ui/albums/state.rs` in shape — concentrated here so the public
//! [`crate::ui::favorites::FavoritesUi`] surface stays small.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use parking_lot::Mutex;

use crate::entities::artist::FavoriteArtist;
use crate::entities::track::{FavoriteStats, MostPlayedFavorite};
use crate::services::settings::{SortDir, ViewSort};
use crate::ui::hero_folds::{HeroFold, MostPlayedTotals};
use crate::ui::row_match::Needle;
use crate::ui::track_list_cache::TrackListCache;

/// Per-section cached snapshots — every fetch lands here so callbacks
/// can recover the underlying Rust data without round-tripping through
/// Slint. Each field is mutated only on its respective fetch path; the
/// `Mutex<>` wrappers keep them callable from any tokio worker without
/// pinning the UI thread.
pub(crate) struct FavoritesUiState {
    /// All favourites, *before* the in-memory search filter, plus the keys
    /// a filter and a sort need. The Slint side renders the post-filter
    /// model; this retains the unfiltered set so a keystroke can re-walk
    /// without hitting `SQLite` — and so a header click can re-sort in
    /// memory rather than re-issuing the query.
    pub tracks_all: TrackListCache,
    /// Active filter needle — cached so the live-refresh subscriber
    /// (after a `library_changed_tx` tick) re-applies it without
    /// re-reading the Slint property from a tokio thread.
    pub filter: Mutex<Needle>,
    /// Active Songs-tab sort — written on `set-sort-*` callbacks, read on
    /// every re-fetch. Default mirrors the Slint global's defaults.
    pub sort: Mutex<ViewSort>,
    /// Active Favorite Artists sort. A second shadow rather than a shared
    /// one: the two tabs sort different entities over disjoint field sets,
    /// and Songs resolves its order in SQL where this one is applied in
    /// memory. Read off the UI thread by `grids::sort::sort_cached_artists`,
    /// which is why it can't just live on the Slint global.
    pub artist_sort: Mutex<ViewSort>,
    /// Hero stats — most recent successful `get_favorite_stats` result.
    /// Held so an empty Slint `mosaic-paths` write on section leave can
    /// be reverted on section re-enter without a DB round trip in the
    /// common case.
    pub stats: Mutex<FavoriteStats>,
    /// Grid-tab rows in Rust shape — kept so click handlers can resolve
    /// `(id) -> entity` without re-fetching, and so a filter keystroke or
    /// a column-count change re-chunks in memory. Two independent caches,
    /// one per grid, refreshed in lockstep on `refresh_grids`.
    pub most_played: Mutex<Vec<MostPlayedFavorite>>,
    pub fav_artists: Mutex<Vec<FavoriteArtist>>,
    /// What the hero band's chips state about the two lists above them, folded
    /// by the fetch that produced the rows — on that worker, before they reach
    /// the cache, never at publish time. `publish_favorites` then reads two
    /// `Copy` words instead of walking every favourite on the UI thread, and a
    /// publish that beats a sibling fetch is stale rather than half-built.
    ///
    /// Both describe the whole set, not the filtered view: the band names the
    /// page, and the counts that do follow the filter are the ones gating the
    /// grids' empty states. See [`crate::ui::hero_chips`].
    ///
    /// Both are reset by `FavoritesUi::release_section_state` alongside the
    /// caches they were folded from — a derived value may not outlive its
    /// source. Dropping that reset doesn't leave the band empty, which would be
    /// the harmless failure; it leaves it stating a spread that no longer
    /// matches the count beside it.
    pub songs_fold: Mutex<HeroFold>,
    pub most_played_totals: Mutex<MostPlayedTotals>,
    /// Set of `TrackListRow.id`s currently `selected: true` on the
    /// Slint model. Same diff-then-write pattern Albums uses to keep
    /// selection updates O(changed) rather than O(rows).
    pub applied_selection: Mutex<HashSet<i32>>,
    /// Mosaic cover paths last composed into the hero blur. Guards against
    /// recomposing (up to 4 decodes + one blur) when a refresh yields the same
    /// covers — the common case for a played-track / in-view-toggle refresh.
    /// Reset on section-leave so a genuine re-enter recomposes.
    pub last_mosaic_paths: Mutex<Vec<String>>,
    /// Hash of the mounted grid's last applied contents, plus the tab and the
    /// column count that shaped them. The same guard `last_mosaic_paths` is,
    /// one surface down: a grid write is a `set_vec` reset that tears down and
    /// rebuilds every mounted card, and a `stats_changed` tick lands on both
    /// tabs while only Most Played is ranked by play count. Reset on
    /// section-leave for the same reason the mosaic's is — the models are
    /// cleared there, so a matching hash would otherwise skip the refill.
    pub last_grid_signature: Mutex<Option<u64>>,
}

impl FavoritesUiState {
    pub(super) fn new() -> Self {
        Self {
            tracks_all: TrackListCache::new(),
            filter: Mutex::new(Needle::default()),
            sort: Mutex::new(ViewSort {
                field: "title".to_owned(),
                dir: SortDir::Asc,
            }),
            artist_sort: Mutex::new(ViewSort {
                field: "favorite_count".to_owned(),
                dir: SortDir::Desc,
            }),
            stats: Mutex::new(FavoriteStats {
                count: 0,
                total_duration_ms: 0,
                artwork_paths: Vec::new(),
            }),
            most_played: Mutex::new(Vec::new()),
            fav_artists: Mutex::new(Vec::new()),
            songs_fold: Mutex::new(HeroFold::default()),
            most_played_totals: Mutex::new(MostPlayedTotals::default()),
            applied_selection: Mutex::new(HashSet::new()),
            last_mosaic_paths: Mutex::new(Vec::new()),
            last_grid_signature: Mutex::new(None),
        }
    }
}

/// Mosaic-tile cache size (px). Each hero tile is rendered at ~70 px
/// (half of the 140 px hero tile, minus the gutter), so 128 px gives a
/// crisp downscale without paying the 384 px detail-tier cost.
pub(super) const MOSAIC_THUMB_SIZE: u32 = 128;

/// LRU capacity for the mosaic-tile cache. Small — at most four covers
/// live in the mosaic at once, but recently-displaced tiles can re-enter
/// when the top-4 most-played shifts. Each buffer is ~48 KiB.
pub(super) const MOSAIC_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(n) => n,
    None => panic!("MOSAIC_THUMB_CAP > 0"),
};

/// LRU capacity per grid tier. Sized like the album grid's default: enough
/// for a screenful or two of cards, so scrolling re-decodes rather than the
/// cache growing with the library.
///
/// The two tiers are never both warm — the tabs are mutually exclusive and
/// `tab-changed` releases the one being left — so this bounds one tier's
/// worth of resident buffers at a time, not two.
///
/// A construction default only: `ui::favorites::tune_cache_for_display`
/// resizes both tiers against the real display once the window is live, the
/// same as the entity grids.
pub(super) const GRID_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("GRID_THUMB_CAP > 0"),
};

/// How many covers to decode up front when a grid tab becomes visible.
///
/// A screenful or two, matching `ui::albums::state::GRID_PREWARM_AHEAD` — not
/// the tier's capacity. The grids are uncapped now, so prewarming everything
/// would decode a large library's whole favourite set at grid size on every
/// `library_changed` tick, and all but the last `GRID_THUMB_CAP_N` of them
/// would be evicted by the prewarm itself before a single card asked for one.
/// The rest decode lazily as rows scroll in, which is what the `ListView`
/// virtualization is for.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
