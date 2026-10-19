//! Internal data structures for the Favorites view. Mirrors `src/ui/albums/state.rs` in shape,
//! concentrated here so the public [`crate::ui::favorites::FavoritesUi`] surface stays small.

use parking_lot::Mutex;

use crate::ui::mosaic_hero::MosaicGuard;
use crate::ui::paged_rows::KeysetTickets;
use melodia_app::services::settings::ViewSort;
use melodia_core::entities::track::FavoriteStats;
use melodia_core::entities::track_list::{MostPlayedTotals, SortDir};
use melodia_core::utils::needle::Needle;

/// Per-section cached snapshots — every fetch lands here so callbacks can recover the underlying
/// Rust data without round-tripping through Slint. Each field is mutated only on its own fetch
/// path; the `Mutex`es keep them callable from any tokio worker without pinning the UI thread.
pub(crate) struct FavoritesUiState {
    /// Active filter needle — cached so the live-refresh subscriber re-applies it without reading
    /// the Slint property from a tokio thread.
    pub filter: Mutex<Needle>,
    /// Active Songs-tab sort, written on `set-sort-*` and read on every re-fetch.
    pub sort: Mutex<ViewSort>,
    /// Orders the Songs tab's keyset loads, a refresh tick, a keystroke and a header click each
    /// taking one.
    pub song_tickets: KeysetTickets,
    /// Most recent successful `get_favorite_stats`, held because `hero_chips::publish_favorites`
    /// takes the band's count, running time and Songs-tab spread off this handle rather than back
    /// off the properties written around it — the page is assembled from three fetches, so no call
    /// site holds all of it.
    pub stats: Mutex<FavoriteStats>,
    /// What the band's chips state about the two grid tabs, answered beside each grid's ids.
    ///
    /// They describe the whole set, not the filtered view: the band names the page, and the count
    /// that does follow the filter is the one gating each grid's empty state. Reset by
    /// `FavoritesUi::release_section_state` beside the grids they describe.
    pub most_played_totals: Mutex<MostPlayedTotals>,
    pub artist_total: Mutex<i32>,
    /// The covers last composed into the banner, guarding against redoing four decodes, a
    /// 600² compose and a blur when a refresh yields the same top four.
    pub last_mosaic_paths: MosaicGuard,
}

impl FavoritesUiState {
    pub(super) fn new() -> Self {
        Self {
            filter: Mutex::new(Needle::default()),
            sort: Mutex::new(ViewSort { field: "title".to_owned(), dir: SortDir::Asc }),
            song_tickets: KeysetTickets::default(),
            stats: Mutex::new(FavoriteStats::default()),
            most_played_totals: Mutex::new(MostPlayedTotals::default()),
            artist_total: Mutex::new(0),
            last_mosaic_paths: MosaicGuard::default(),
        }
    }
}
