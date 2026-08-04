//! Internal data structures for the Recently-Played view. Trimmed sibling of
//! `src/ui/favorites/state.rs` — one grid tab rather than two, and no sort state
//! at all: the Songs list keeps its recency fetch order and Most Played's title
//! names its own, so the cached sets are only ever filtered.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use parking_lot::Mutex;

use crate::entities::track::{MostPlayedFavorite, TrackListRow as RsTrackListRow};
use crate::ui::hero_chips::{HeroFold, MostPlayedTotals};
use crate::ui::row_match::Needle;

/// What the Songs tab's band states about the recency set.
///
/// Favorites keeps the same pair on the `FavoriteStats` its hero query returns.
/// There is no stats query here — the numbers are summed off the recency rows
/// themselves — so they need somewhere of their own to live, and it has to be
/// somewhere that outlives the fetch: the band is per-tab now, so a publish
/// triggered from the *grid* still has to be able to state them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct SongsTotals {
    pub tracks: i32,
    pub duration_ms: i64,
}

/// Per-section cached snapshots — every fetch lands here so callbacks can
/// recover the underlying Rust data without round-tripping through Slint.
pub(crate) struct RecentlyPlayedUiState {
    /// The most-recently-played rows in fetch (recency) order, *before* the
    /// in-memory search filter. Both membership and order are fixed to this
    /// set — the view never re-queries and never re-orders.
    pub tracks_all: Mutex<Vec<RsTrackListRow>>,
    /// Active filter needle — cached so the live-refresh subscriber can
    /// re-apply it off the UI thread. Shared by both tabs.
    pub filter: Mutex<Needle>,
    /// Most Played rows in Rust shape — kept so click handlers can resolve
    /// `(id) -> entity` without re-fetching, and so a filter keystroke or a
    /// column-count change re-chunks in memory.
    pub most_played: Mutex<Vec<MostPlayedFavorite>>,
    /// What the band's chips state about the two tabs above them, folded by the
    /// fetch that produced the rows — on that worker, before they reach the
    /// cache, never at publish time. `publish_recently_played` then reads `Copy`
    /// words instead of walking a play history on the UI thread, and a publish
    /// that beats a sibling fetch is stale rather than half-built.
    ///
    /// All three describe the whole set, not the filtered view: the band names
    /// the page, and the count that does follow the filter is the one gating the
    /// grid's empty state. See [`crate::ui::hero_chips`].
    ///
    /// All three are reset by `RecentlyPlayedUi::release_section_state` alongside
    /// the caches they were folded from — a derived value may not outlive its
    /// source. Dropping that reset doesn't leave the band empty, which would be
    /// the harmless failure; it leaves it stating a spread that no longer matches
    /// the count beside it.
    pub songs_totals: Mutex<SongsTotals>,
    pub songs_fold: Mutex<HeroFold>,
    pub most_played_totals: Mutex<MostPlayedTotals>,
    /// Set of `TrackListRow.id`s currently `selected: true` on the Slint
    /// model. Same diff-then-write pattern the other list views use.
    pub applied_selection: Mutex<HashSet<i32>>,
    /// Mosaic cover paths last composed into the hero blur. Guards against
    /// recomposing (up to 4 decodes + one blur) when a refresh yields the same
    /// top-4 covers — the common case for a played-track / in-view-toggle
    /// refresh. Reset on section-leave so a genuine re-enter recomposes.
    pub last_mosaic_paths: Mutex<Vec<String>>,
    /// Hash of the Most Played grid's last applied contents, plus the tab and the
    /// column count that shaped them. The same guard `last_mosaic_paths` is, one
    /// surface down: a grid write is a `set_vec` reset that tears down and
    /// rebuilds every mounted card, and a `stats_changed` tick lands on both tabs
    /// while only this one is ranked by play count. Reset on section-leave for
    /// the same reason the mosaic's is — the models are cleared there, so a
    /// matching hash would otherwise skip the refill.
    pub last_grid_signature: Mutex<Option<u64>>,
}

impl RecentlyPlayedUiState {
    pub(super) fn new() -> Self {
        Self {
            tracks_all: Mutex::new(Vec::new()),
            filter: Mutex::new(Needle::default()),
            most_played: Mutex::new(Vec::new()),
            songs_totals: Mutex::new(SongsTotals::default()),
            songs_fold: Mutex::new(HeroFold::default()),
            most_played_totals: Mutex::new(MostPlayedTotals::default()),
            applied_selection: Mutex::new(HashSet::new()),
            last_mosaic_paths: Mutex::new(Vec::new()),
            last_grid_signature: Mutex::new(None),
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

/// Most-Played grid tile size (px). This was a 180 px tier sized for 160 px
/// strip cards; the tab draws the same flex-filled cards the Albums and Artists
/// grids do, which run well past 260 px, and `FemtoVG` minifies with plain
/// bilinear (no mipmaps) — so an undersized tier is a visibly soft tile rather
/// than a saving. Matches `ui::favorites::state::GRID_THUMB_SIZE`.
pub(super) const MOST_PLAYED_THUMB_SIZE: u32 = 448;

/// LRU capacity for the Most Played tier. Sized like the album grid's default:
/// enough for a screenful or two of cards, so scrolling re-decodes rather than
/// the cache growing with the library.
///
/// A construction default only: [`super::tune_cache_for_display`] resizes it
/// against the real display once the window is live, the same as the entity
/// grids and both Favorites grid tabs.
pub(super) const GRID_THUMB_CAP: NonZeroUsize = match NonZeroUsize::new(48) {
    Some(n) => n,
    None => panic!("GRID_THUMB_CAP > 0"),
};

/// How many covers to decode up front when the Most Played tab becomes visible.
///
/// A screenful or two, matching `ui::favorites::state::GRID_PREWARM_AHEAD` — not
/// the tier's capacity. The grid is uncapped, so prewarming everything would
/// decode a large library's whole played set at 448 px on every `stats_changed`
/// tick, and all but the last capacity's worth would be evicted by the prewarm
/// itself before a single card asked for one. The rest decode lazily as rows
/// scroll in, which is what the `ListView` virtualization is for.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
