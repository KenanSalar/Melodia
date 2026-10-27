//! The Recently Played page: a hero over two tabs. Songs is the capped recency list, fetched once
//! per refresh and filtered in memory, in an order that is the page and so never sorts; Most
//! Played is a paged grid of the library's played tracks, filtered in SQL.

mod callbacks;
mod grid;
mod hero;
mod rows;
mod songs;
mod state;
mod tabs;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::ui::artwork_cache::BlurSpec;
use crate::ui::card_grid::GridLoads;
use crate::ui::detail_artwork;
use crate::ui::hero_folds::HeroFold;
use crate::ui::paged_rows::ScrollPoint;
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::view_ctx::ViewCtx;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track_list::MostPlayedTotals;

use state::{RecentlyPlayedUiState, SongsTotals};

/// This page's `Nav.selected-index`, and the single definition of it.
pub const NAV_RECENTLY_PLAYED: i32 = 8;

pub(super) use grid::{
    most_played_ids, prewarm_covers, release_grid, reload_grid, requery_grid, set_columns,
};
pub(super) use songs::{
    apply_filtered_tracks, apply_filtered_tracks_now, apply_row_favorite, apply_row_rating,
    current_filter, refresh_tracks, set_filter,
};
pub use tabs::RecentlyPlayedTab;
pub(super) use tabs::{seed_tab, tab_from_index};

/// Wires every `RecentlyPlayed.*` callback and seeds the persisted tab, here for [`seed_tab`]'s
/// reason.
pub fn install(cx: ViewCtx<'_>) -> Arc<RecentlyPlayedUi> {
    let most_played_scroll = rows::install_recently_played_models(cx.app, cx.state);
    let rp_ui = Arc::new(RecentlyPlayedUi::new(
        cx.cover_thumbs.clone(),
        detail_artwork::blur_spec(cx.app),
        most_played_scroll,
    ));
    callbacks::wire(cx.app, cx.state, &rp_ui);
    if let Some(vs) = cx.view_state {
        seed_tab(cx.app, &rp_ui, vs.recently_played_tab);
    }
    rp_ui
}

/// The Recently Played page's handle, shared by its callbacks and its off-thread loads.
pub struct RecentlyPlayedUi {
    inner: RecentlyPlayedUiState,
    /// The shared row tier, for the Songs tab's `TrackList` column.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The band's blur shape, `None` under the aurora setting. Read at install, the compose
    /// running where no window is in reach.
    pub(super) hero_blur: Option<BlurSpec>,
    pub(super) most_played: GridLoads,
    section: SectionState,
    /// Shadow of `RecentlyPlayed.tab-idx` for the off-thread loads, which skip the tab not mounted.
    active_tab: AtomicU8,
    /// [`SectionState`]'s dirty flag for Most Played alone, the page having one `SectionActiveGate`
    /// for both tabs: a tick skipped on Songs is remembered here for the pick that mounts the grid.
    /// Seeded `true`, so the first pick loads.
    grid_dirty: AtomicBool,
}

impl RecentlyPlayedUi {
    fn new(
        cover_thumbs: Arc<CoverThumbs>,
        hero_blur: Option<BlurSpec>,
        most_played_scroll: ScrollPoint,
    ) -> Self {
        Self {
            inner: RecentlyPlayedUiState::new(),
            cover_thumbs,
            hero_blur,
            most_played: GridLoads::new(most_played_scroll),
            section: SectionState::new(),
            active_tab: AtomicU8::new(RecentlyPlayedTab::Songs.as_code()),
            grid_dirty: AtomicBool::new(true),
        }
    }

    pub fn set_active_tab(&self, tab: RecentlyPlayedTab) {
        self.active_tab.store(tab.as_code(), Ordering::Relaxed);
    }

    pub fn active_tab(&self) -> RecentlyPlayedTab {
        RecentlyPlayedTab::from_code(self.active_tab.load(Ordering::Relaxed))
    }

    /// Remember that the Most Played grid missed a tick, or was left showing what a pick undoes.
    pub fn mark_grid_dirty(&self) {
        self.grid_dirty.store(true, Ordering::Release);
    }

    /// `true` iff the Most Played grid has missed a tick since its last load.
    pub fn take_grid_dirty(&self) -> bool {
        self.grid_dirty.swap(false, Ordering::AcqRel)
    }

    /// Serializes the leave's wipe against a store. Never held across an `.await`.
    pub(super) fn gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.section.gate()
    }

    /// Forgets the collage recorded as on screen; see
    /// [`crate::ui::favorites::FavoritesUi::forget_mosaic`].
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.forget();
    }

    /// Drops every section-local buffer, as
    /// [`crate::ui::favorites::FavoritesUi::release_section_state`] does.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        crate::ui::grid_prewarm::shrink_covers();
        {
            let _gate = self.gate();
            self.inner.tracks_all.lock().clear();
            // The folds go with the sets they summarise: outliving them, the band would state
            // something wrong rather than nothing.
            *self.inner.songs_totals.lock() = SongsTotals::default();
            *self.inner.songs_fold.lock() = HeroFold::default();
            *self.inner.most_played_totals.lock() = MostPlayedTotals::default();
        }
        // The grid is empty now, so the pick that mounts it owes a load.
        self.mark_grid_dirty();
        melodia_platform::services::platform::allocator::trim();
    }

    pub(crate) fn state(&self) -> &RecentlyPlayedUiState {
        &self.inner
    }

    /// The Songs tab's ids as displayed, so `shuffle-all` and `play-row` queue what the user sees.
    pub fn filtered_track_ids(&self) -> Vec<i64> {
        let needle = self.inner.filter.lock().clone();
        self.inner
            .tracks_all
            .lock()
            .iter()
            .filter(|r| crate::ui::row_match::track_matches(r, &needle))
            .map(|r| r.id)
            .collect()
    }

    /// Patches the cached row, which stays: recency membership doesn't depend on the flag.
    pub fn flip_track_favorite(&self, id: i64, fav: bool) {
        if let Some(r) = self.inner.tracks_all.lock().iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
    }

    /// [`Self::flip_track_favorite`]'s star-rating twin.
    pub fn flip_track_rating(&self, id: i64, rating: i32) {
        if let Some(r) = self.inner.tracks_all.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
    }
}

impl_section_state_helpers!(RecentlyPlayedUi);

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<RecentlyPlayedUi>();
};

#[cfg(test)]
#[path = "tests/recently_played_tests.rs"]
mod tests;
