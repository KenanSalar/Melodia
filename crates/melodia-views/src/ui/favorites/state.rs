//! What [`crate::ui::favorites::FavoritesUi`] caches.

use parking_lot::Mutex;

use crate::ui::mosaic_hero::MosaicGuard;
use melodia_app::services::settings::ViewSort;
use melodia_core::entities::track::FavoriteStats;
use melodia_core::entities::track_list::{MostPlayedTotals, SortDir};
use melodia_core::utils::needle::Needle;

/// What the fetches cache, so a worker reads Rust data rather than a Slint property.
pub(crate) struct FavoritesUiState {
    pub filter: Mutex<Needle>,
    /// The Songs tab's sort.
    pub sort: Mutex<ViewSort>,
    /// Held for `hero_chips::publish_favorites`, which reads the band's facts off this handle: the
    /// page is assembled from three fetches, so no call site holds all of them.
    pub stats: Mutex<FavoriteStats>,
    /// What the band states about the two grid tabs: the whole set rather than the filtered view,
    /// the band naming the page.
    pub most_played_totals: Mutex<MostPlayedTotals>,
    pub artist_total: Mutex<i32>,
    /// The covers last composed into the banner, so an unchanged top four skips the compose.
    pub last_mosaic_paths: MosaicGuard,
}

impl FavoritesUiState {
    pub(super) fn new() -> Self {
        Self {
            filter: Mutex::new(Needle::default()),
            sort: Mutex::new(ViewSort { field: "title".to_owned(), dir: SortDir::Asc }),
            stats: Mutex::new(FavoriteStats::default()),
            most_played_totals: Mutex::new(MostPlayedTotals::default()),
            artist_total: Mutex::new(0),
            last_mosaic_paths: MosaicGuard::default(),
        }
    }
}
