//! Internal data structures for the Recently-Played view. Trimmed sibling of
//! `src/ui/favorites/state.rs`, with no sort state at all: the Songs list keeps its recency fetch
//! order and Most Played's title names its own.

use parking_lot::Mutex;

use crate::ui::hero_folds::HeroFold;
use crate::ui::mosaic_hero::MosaicGuard;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::entities::track_list::MostPlayedTotals;
use melodia_core::utils::needle::Needle;

/// What the Songs tab's band states about the recency set.
///
/// Favorites keeps the same pair on the `FavoriteStats` its hero query returns; there
/// is no stats query here, so the numbers are summed off the recency rows and need
/// somewhere that outlives the fetch — the band is per-tab, so a publish triggered
/// from the *grid* still has to be able to state them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct SongsTotals {
    pub tracks: i32,
    pub duration_ms: i64,
}

/// Per-section cached snapshots — every fetch lands here so callbacks can
/// recover the underlying Rust data without round-tripping through Slint.
pub(crate) struct RecentlyPlayedUiState {
    /// The most-recently-played rows in fetch order, *before* the in-memory filter.
    /// Membership and order are both fixed to this set — the view never re-queries
    /// and never re-orders.
    pub tracks_all: Mutex<Vec<RsTrackListRow>>,
    /// Active filter needle — cached so the live-refresh subscriber can re-apply it
    /// off the UI thread. Shared by both tabs.
    pub filter: Mutex<Needle>,
    /// What the band's chips state about the two tabs above them, answered by the
    /// load that produced the rows rather than at publish time — so
    /// `publish_recently_played` reads `Copy` words instead of walking a play history
    /// on the UI thread, and a publish that beats a sibling load is stale rather
    /// than half-built.
    ///
    /// All three describe the whole set, not the filtered view: the band names the
    /// page, and the count that does follow the filter is the one gating the grid's
    /// empty state. All three are reset by
    /// `RecentlyPlayedUi::release_section_state` beside the caches they were folded
    /// from. Dropping that reset doesn't leave the band empty, which would be the
    /// harmless failure, but leaves it stating a spread that no longer matches the
    /// count beside it.
    pub songs_totals: Mutex<SongsTotals>,
    pub songs_fold: Mutex<HeroFold>,
    pub most_played_totals: Mutex<MostPlayedTotals>,
    /// The covers last composed into the banner, guarding against redoing four decodes, a
    /// 600² compose and a blur when a refresh yields the same top four — the common case.
    pub last_mosaic_paths: MosaicGuard,
}

impl RecentlyPlayedUiState {
    pub(super) fn new() -> Self {
        Self {
            tracks_all: Mutex::new(Vec::new()),
            filter: Mutex::new(Needle::default()),
            songs_totals: Mutex::new(SongsTotals::default()),
            songs_fold: Mutex::new(HeroFold::default()),
            most_played_totals: Mutex::new(MostPlayedTotals::default()),
            last_mosaic_paths: MosaicGuard::default(),
        }
    }
}
