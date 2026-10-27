//! The Favorites page: a hero over three tabs, Songs, Most Played and Favorite Artists, each
//! searched and sorted in SQL. There is no Favorite Albums tab, the Albums page covering that.
//!
//! The mosaic is the first four distinct covers of Most Played's own ranking, so the two can't
//! disagree about a tie, topped up from the favourites that tab excludes.
//!
//! The shared contract is `.claude/rules/ui-patterns.md`'s "three tabbed pages" block.

mod callbacks;
mod grids;
mod hero;
mod rows;
mod songs;
mod state;
mod tabs;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use slint::{ComponentHandle, Global as _, Weak};

use crate::ui::artists::ArtistsUi;
use crate::ui::artwork_cache::BlurSpec;
use crate::ui::card_grid::GridLoads;
use crate::ui::detail_artwork;
use crate::ui::paged_rows::track_rows::ListLoads;
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::view_ctx::ViewCtx;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track::FavoriteStats;
use melodia_core::entities::track_list::MostPlayedTotals;
use melodia_ui::Favorites;

use rows::Scrolls;
use state::FavoritesUiState;

/// This page's `Nav.selected-index`, and the single definition of it.
pub const NAV_FAVORITES: i32 = 2;

pub(super) use grids::{prewarm_tab_covers, release_grids, reload_grid, requery_grid, set_columns};
pub(super) use hero::refresh_hero;
pub(super) use rows::{most_played_ids, to_slint_fav_artist_row};
pub(super) use songs::{
    current_filter, displayed_ids, release_tracks, requery_tracks, set_filter, set_sort,
};
pub use tabs::FavoritesTab;
pub(super) use tabs::{seed_tab, tab_from_index};

/// Wires every `Favorites.*` callback and seeds the persisted tab, here for [`seed_tab`]'s reason.
/// `artists_ui` is the open-artist hand-off, a parameter so the install order is a compile error.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>, artists_ui: &Arc<ArtistsUi>) -> Arc<FavoritesUi> {
    let scrolls = rows::install_favorites_models(cx.app, cx.state);
    let favorites_ui = Arc::new(FavoritesUi::new(
        cx.cover_thumbs.clone(),
        detail_artwork::blur_spec(cx.app),
        scrolls,
        cx.app.global::<Favorites>().as_weak(),
    ));
    callbacks::wire(cx.app, cx.state, cx.view_state, &favorites_ui, artists_ui);
    if let Some(vs) = cx.view_state {
        seed_tab(cx.app, &favorites_ui, vs.favorites_tab);
    }
    favorites_ui
}

/// The Favorites page's handle, shared by its callbacks and its off-thread loads.
pub struct FavoritesUi {
    inner: FavoritesUiState,
    /// The shared row tier, for the Songs tab's `TrackList` column.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The band's blur shape, `None` under the aurora setting. Read at install, the compose
    /// running where no window is in reach.
    pub(super) hero_blur: Option<BlurSpec>,
    global: Weak<Favorites<'static>>,
    songs: ListLoads,
    pub(super) most_played: GridLoads,
    pub(super) artists: GridLoads,
    section: SectionState,
    /// Shadow of `Favorites.tab-idx` for the off-thread loads, which skip the tabs not mounted.
    active_tab: AtomicU8,
    /// [`SectionState`]'s dirty flag per tab, the page having one `SectionActiveGate` for all
    /// three. Seeded `true`, so the first pick onto a tab loads. Sound only while `refresh_hero`,
    /// which the band reads on every tab, stays ungated.
    songs_dirty: AtomicBool,
    most_played_dirty: AtomicBool,
    artists_dirty: AtomicBool,
}

impl FavoritesUi {
    fn new(
        cover_thumbs: Arc<CoverThumbs>,
        hero_blur: Option<BlurSpec>,
        scrolls: Scrolls,
        global: Weak<Favorites<'static>>,
    ) -> Self {
        Self {
            inner: FavoritesUiState::new(),
            cover_thumbs,
            hero_blur,
            global,
            songs: ListLoads::new(scrolls.songs),
            most_played: GridLoads::new(scrolls.most_played),
            artists: GridLoads::new(scrolls.artists),
            section: SectionState::new(),
            active_tab: AtomicU8::new(FavoritesTab::Songs.as_code()),
            songs_dirty: AtomicBool::new(true),
            most_played_dirty: AtomicBool::new(true),
            artists_dirty: AtomicBool::new(true),
        }
    }

    pub fn set_active_tab(&self, tab: FavoritesTab) {
        self.active_tab.store(tab.as_code(), Ordering::Relaxed);
    }

    pub fn active_tab(&self) -> FavoritesTab {
        FavoritesTab::from_code(self.active_tab.load(Ordering::Relaxed))
    }

    /// Whether the mounted tab is filtered, which a pick clearing the shared needle leaves out of
    /// date.
    pub fn shows_filtered(&self) -> bool {
        !current_filter(self).is_empty()
    }

    fn dirty(&self, tab: FavoritesTab) -> &AtomicBool {
        match tab {
            FavoritesTab::Songs => &self.songs_dirty,
            FavoritesTab::MostPlayed => &self.most_played_dirty,
            FavoritesTab::Artists => &self.artists_dirty,
        }
    }

    /// Remember that `tab` missed a tick, or was left showing what a pick undoes.
    pub fn mark_tab_dirty(&self, tab: FavoritesTab) {
        self.dirty(tab).store(true, Ordering::Release);
    }

    /// `true` iff `tab` has missed a tick since its last load.
    pub fn take_tab_dirty(&self, tab: FavoritesTab) -> bool {
        self.dirty(tab).swap(false, Ordering::AcqRel)
    }

    pub fn mark_songs_dirty(&self) {
        self.mark_tab_dirty(FavoritesTab::Songs);
    }

    /// Serializes the leave's wipe against a store. Never held across an `.await`.
    pub(super) fn gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.section.gate()
    }

    /// Forgets the collage recorded as on screen, so the next refresh recomposes. Called beside the
    /// leave's unconditional hero-slot wipe rather than from [`Self::release_section_state`], which
    /// bails on a quick return and would leave the guard set over a hero with no artwork.
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.forget();
    }

    /// Drops every section-local buffer, off the UI thread once the leave has released the lists
    /// and grids. The gate serializes the wipe against a store without ordering the two: what stops
    /// a late fetch refilling it is each fetcher's own `section_active()` bail, and for the grids
    /// the revoke on their release.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        crate::ui::grid_prewarm::shrink_covers();
        {
            let _gate = self.gate();
            *self.inner.stats.lock() = FavoriteStats::default();
            // The totals go with the grids they summarise: outliving them, the band would state
            // something wrong rather than nothing.
            *self.inner.most_played_totals.lock() = MostPlayedTotals::default();
            *self.inner.artist_total.lock() = 0;
        }
        // Every list and grid is empty now, so whichever tab is entered next owes a load.
        for tab in FavoritesTab::ALL {
            self.mark_tab_dirty(tab);
        }
        melodia_platform::services::platform::allocator::trim();
    }

    pub(crate) fn state(&self) -> &FavoritesUiState {
        &self.inner
    }
}

impl_section_state_helpers!(FavoritesUi);

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<FavoritesUi>();
};

#[cfg(test)]
#[path = "tests/favorites_tests.rs"]
mod tests;
