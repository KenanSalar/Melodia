//! What [`crate::ui::recently_played::RecentlyPlayedUi`] caches. There is no sort state: recency
//! is the Songs list's order, and Most Played's title names its own.

use parking_lot::Mutex;

use crate::ui::hero_folds::HeroFold;
use crate::ui::mosaic_hero::MosaicGuard;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::entities::track_list::MostPlayedTotals;
use melodia_core::utils::needle::Needle;

/// What the band states about the recency set, summed off its rows and kept past the fetch for a
/// publish from the grid to state too.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct SongsTotals {
    pub tracks: i32,
    pub duration_ms: i64,
}

/// What the fetches cache, so a worker reads Rust data rather than a Slint property.
pub(crate) struct RecentlyPlayedUiState {
    /// The recency rows in fetch order, before the filter.
    pub tracks_all: Mutex<Vec<RsTrackListRow>>,
    /// Shared by both tabs.
    pub filter: Mutex<Needle>,
    /// What the band states about both tabs, folded by the load that produced the rows, so a
    /// publish reads `Copy` words rather than walking a play history on the UI thread. The whole
    /// set rather than the filtered view, the band naming the page.
    pub songs_totals: Mutex<SongsTotals>,
    pub songs_fold: Mutex<HeroFold>,
    pub most_played_totals: Mutex<MostPlayedTotals>,
    /// The covers last composed into the banner, so an unchanged top four skips the compose.
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
