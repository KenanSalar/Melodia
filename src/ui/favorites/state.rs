//! Internal data structures for the Favorites view. Mirrors `src/ui/albums/state.rs`
//! in shape, concentrated here so the public [`crate::ui::favorites::FavoritesUi`]
//! surface stays small.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use parking_lot::Mutex;

use crate::entities::artist::FavoriteArtist;
use crate::entities::track::{FavoriteStats, MostPlayedFavorite};
use crate::services::settings::{SortDir, ViewSort};
use crate::ui::hero_folds::{HeroFold, MostPlayedTotals};
use crate::ui::row_match::Needle;
use crate::ui::track_list_cache::TrackListCache;

/// Per-section cached snapshots — every fetch lands here so callbacks can recover the
/// underlying Rust data without round-tripping through Slint. Each field is mutated
/// only on its own fetch path; the `Mutex`es keep them callable from any tokio worker
/// without pinning the UI thread.
pub(crate) struct FavoritesUiState {
    /// All favourites *before* the in-memory filter, plus the keys a filter and a sort
    /// need. Slint renders the post-filter model; retaining the unfiltered set is what
    /// lets a keystroke re-walk without hitting `SQLite` and a header click re-sort in
    /// memory rather than re-issuing the query.
    pub tracks_all: TrackListCache,
    /// Active filter needle — cached so the live-refresh subscriber re-applies it
    /// without reading the Slint property from a tokio thread.
    pub filter: Mutex<Needle>,
    /// Active Songs-tab sort, written on `set-sort-*` and read on every re-fetch. The
    /// default mirrors the Slint global's.
    pub sort: Mutex<ViewSort>,
    /// Active Favorite Artists sort. A second shadow rather than a shared one: the two
    /// tabs sort different entities over disjoint field sets, and Songs resolves its
    /// order in SQL where this one is applied in memory. Read off the UI thread by
    /// `grids::sort::sort_cached_artists`, which is why it can't live on the global.
    pub artist_sort: Mutex<ViewSort>,
    /// Most recent successful `get_favorite_stats`, held so the empty `mosaic-paths`
    /// write on section leave can be reverted on re-enter without a DB round trip.
    pub stats: Mutex<FavoriteStats>,
    /// Grid-tab rows in Rust shape, so click handlers resolve `(id) -> entity` without
    /// re-fetching and a keystroke or column-count change re-chunks in memory. Two
    /// independent caches refreshed in lockstep on `refresh_grids`.
    pub most_played: Mutex<Vec<MostPlayedFavorite>>,
    pub fav_artists: Mutex<Vec<FavoriteArtist>>,
    /// What the hero band's chips state about the two lists above them, folded on the
    /// worker that produced the rows rather than at publish time — so
    /// `publish_favorites` reads two `Copy` words instead of walking every favourite on
    /// the UI thread, and a publish that beats a sibling fetch is stale rather than
    /// half-built.
    ///
    /// Both describe the whole set, not the filtered view: the band names the page, and
    /// the counts that do follow the filter are the ones gating the grids' empty
    /// states. Both are reset by `FavoritesUi::release_section_state` beside the caches
    /// they were folded from — dropping that reset doesn't leave the band empty, which
    /// would be the harmless failure, but leaves it stating a spread that no longer
    /// matches the count beside it.
    pub songs_fold: Mutex<HeroFold>,
    pub most_played_totals: Mutex<MostPlayedTotals>,
    /// `TrackListRow.id`s currently `selected: true` on the Slint model. The same
    /// diff-then-write Albums uses to keep updates O(changed) rather than O(rows).
    pub applied_selection: Mutex<HashSet<i32>>,
    /// Mosaic cover paths last composed into the hero blur, guarding against
    /// recomposing four decodes and a blur when a refresh yields the same covers — the
    /// common case. Reset on section-leave so a genuine re-enter recomposes.
    pub last_mosaic_paths: Mutex<Vec<String>>,
    /// Hash of the mounted grid's last applied contents plus the tab and column count
    /// that shaped them. The same guard one surface down: a grid write is a `set_vec`
    /// reset that rebuilds every mounted card, and a `stats_changed` tick lands on both
    /// tabs while only Most Played ranks by play count. Reset on section-leave, the
    /// models being cleared there, so a matching hash would otherwise skip the refill.
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

/// Mosaic-tile cache size (px). A tile renders at ~70 px — half the 140 px hero
/// square, minus the gutter — so 128 px downscales crisply without paying the 384 px
/// detail tier.
pub(super) const MOSAIC_THUMB_SIZE: u32 = 128;

/// LRU capacity for the mosaic-tile cache. Small: at most four covers live in the
/// mosaic at once, with headroom for displaced tiles re-entering as the top-4 shifts.
pub(super) const MOSAIC_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(n) => n,
    None => panic!("MOSAIC_THUMB_CAP > 0"),
};

/// LRU capacity per grid tier, sized like the album grid's default: a screenful or
/// two of cards, so scrolling re-decodes rather than the cache growing with the
/// library. The two tiers are never both warm — the tabs are mutually exclusive and
/// `tab-changed` releases the one being left — so this bounds one tier at a time, not
/// two. A construction default only: `ui::favorites::tune_cache_for_display` resizes
/// both against the real display once the window is live.
pub(super) const GRID_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("GRID_THUMB_CAP > 0"),
};

/// How many covers to decode up front when a grid tab becomes visible.
///
/// A screenful or two, matching `ui::albums::state::GRID_PREWARM_AHEAD` and not the
/// tier's capacity. The grids are uncapped, so prewarming everything would decode a
/// large library's whole favourite set on every `library_changed` tick and evict all
/// but the last capacity's worth before a card asked for one.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
