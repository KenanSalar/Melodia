//! Internal data structures for the Favorites view. Mirrors `src/ui/albums/state.rs` in shape,
//! concentrated here so the public [`crate::ui::favorites::FavoritesUi`] surface stays small.

use parking_lot::Mutex;

use crate::ui::hero_folds::MostPlayedTotals;
use crate::ui::mosaic_hero::MosaicGuard;
use crate::ui::paged_rows::KeysetTickets;
use melodia_app::services::settings::ViewSort;
use melodia_core::entities::artist::FavoriteArtist;
use melodia_core::entities::track::{FavoriteStats, MostPlayedFavorite};
use melodia_core::entities::track_list::SortDir;
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
    /// Active Favorite Artists sort. A second shadow rather than a shared one: the two tabs sort
    /// different entities over disjoint field sets. Read off the UI thread by
    /// `grids::sort::sort_cached_artists`, which is why it can't live on the global.
    pub artist_sort: Mutex<ViewSort>,
    /// Most recent successful `get_favorite_stats`, held because `hero_chips::publish_favorites`
    /// takes the band's count, running time and Songs-tab spread off this handle rather than back
    /// off the properties written around it — the page is assembled from three fetches, so no call
    /// site holds all of it.
    pub stats: Mutex<FavoriteStats>,
    /// Grid-tab rows in Rust shape, so click handlers resolve `(id) -> entity` without re-fetching
    /// and a keystroke or column-count change re-chunks in memory. Refreshed in lockstep on
    /// `refresh_grids`.
    pub most_played: Mutex<Vec<MostPlayedFavorite>>,
    pub fav_artists: Mutex<Vec<FavoriteArtist>>,
    /// What the hero band's chips state about the Most Played grid, folded on the worker that
    /// produced the rows rather than at publish time, so `publish_favorites` reads `Copy` words
    /// instead of walking every favourite on the UI thread.
    ///
    /// It describes the whole set, not the filtered view: the band names the page, and the count
    /// that does follow the filter is the one gating the grid's empty state. Reset by
    /// `FavoritesUi::release_section_state` beside the cache it was folded from.
    pub most_played_totals: Mutex<MostPlayedTotals>,
    /// The covers last composed into the banner, guarding against redoing four decodes, a
    /// 600² compose and a blur when a refresh yields the same top four.
    pub last_mosaic_paths: MosaicGuard,
    /// Hash of the mounted grid's last applied contents plus the tab and column count that shaped
    /// them. The same guard one surface down: a grid write is a `set_vec` reset that rebuilds
    /// every mounted card. Reset on section-leave, the models being cleared there, so a matching
    /// hash would otherwise skip the refill.
    pub last_grid_signature: Mutex<Option<u64>>,
}

impl FavoritesUiState {
    pub(super) fn new() -> Self {
        Self {
            filter: Mutex::new(Needle::default()),
            sort: Mutex::new(ViewSort { field: "title".to_owned(), dir: SortDir::Asc }),
            song_tickets: KeysetTickets::default(),
            artist_sort: Mutex::new(ViewSort {
                field: "favorite_count".to_owned(),
                dir: SortDir::Desc,
            }),
            stats: Mutex::new(FavoriteStats::default()),
            most_played: Mutex::new(Vec::new()),
            fav_artists: Mutex::new(Vec::new()),
            most_played_totals: Mutex::new(MostPlayedTotals::default()),
            last_mosaic_paths: MosaicGuard::default(),
            last_grid_signature: Mutex::new(None),
        }
    }
}

/// How many covers to decode up front when a grid tab becomes visible.
///
/// A screenful or two, matching `ui::albums::state::GRID_PREWARM_AHEAD` and **not** the tier's
/// capacity. The grids are uncapped, so prewarming everything would decode a large library's whole
/// favourite set every tick and evict all but the last capacity's worth before a card asked.
pub(super) const GRID_PREWARM_AHEAD: usize = 24;
