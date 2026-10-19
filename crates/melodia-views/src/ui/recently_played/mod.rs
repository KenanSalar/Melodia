//! The Recently Played page: a `MosaicTabHero` banner over two tabs.
//!
//! Songs is a capped `TrackList` of the most-recently-played tracks, fetched once per refresh and
//! re-walked in memory on a keystroke. It mounts `sortable: false`: recency *is* the page, so its
//! headers resize and toggle but never re-order. Most Played is a paged grid of the library's
//! played tracks by count, filtered in SQL.
//!
//! The page's shared contract is `.claude/rules/ui-patterns.md`'s "three tabbed pages" block; the
//! deltas below are this page's.
//!
//! The tree is split by the question each file answers rather than by tab. This file is the handle
//! and its teardown; `tabs.rs` the sub-view enum and its seeding, `rows.rs` the Slint models,
//! `hero.rs` the band, `songs.rs` the Songs tab, and `grid.rs` the Most Played tab.

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

/// This page's `Nav.selected-index` — see [`crate::ui::favorites::NAV_FAVORITES`].
pub const NAV_RECENTLY_PLAYED: i32 = 8;

// `pub(super)` is `pub(in crate::ui)` here, exactly the reach these need: this slice's own
// `callbacks/`, plus `ui::hero_chips`.
pub(super) use grid::{
    most_played_ids, prewarm_covers, release_grid, reload_grid, requery_grid, set_columns,
};
pub(super) use songs::{
    apply_filtered_tracks, apply_filtered_tracks_now, apply_row_favorite, apply_row_rating,
    current_filter, refresh_tracks, set_filter,
};
pub use tabs::RecentlyPlayedTab;
pub(super) use tabs::{seed_tab, tab_from_index};

/// Install the Recently-Played models, build the handle, wire every `RecentlyPlayed.*` callback,
/// and seed the persisted tab.
///
/// A trimmed Favorites. Its row-menu "Go to …" entries are wired centrally by
/// `wire_cross_tab_nav`, so it takes no peer handle; the tab seed folds in here for the reason it
/// does there — see [`crate::ui::favorites::install`].
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

/// Rust-side state for the Recently Played view, shared between the UI callbacks and the async
/// fetchers behind an `Arc`.
pub struct RecentlyPlayedUi {
    inner: RecentlyPlayedUiState,
    /// The shared row tier, for the Songs tab's `TrackList` column.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The band's blur shape, or `None` under the aurora setting — read once at install, the
    /// setting being restart-gated, because the compose runs where no window handle is in reach.
    pub(super) hero_blur: Option<BlurSpec>,
    /// What the Most Played grid's loads share.
    pub(super) most_played: GridLoads,
    /// Visibility + staleness + the mutation gate, the unit every entity grid carries. Also gates
    /// the refresh subscriber, so a background tick can't repaint a hidden view.
    section: SectionState,
    /// Synchronous shadow of `RecentlyPlayed.tab-idx`. The off-thread loads read which tab is
    /// mounted from it — only one is, so loading or warming the other is work no frame draws.
    active_tab: AtomicU8,
    /// [`SectionState`]'s dirty flag one level down, for Most Played alone.
    ///
    /// **The section flag can't answer this, the page having one `SectionActiveGate` where My
    /// Library has one per tab.** Ungated, the grid reloaded on every `stats_changed` tick while
    /// the user read Songs. This is where a skipped tick is remembered, so the pick that mounts the
    /// grid knows to reload. Seeded `true`, so the first pick always does.
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

    /// Mirror the mounted sub-view. Written on the UI thread, from the tab bar's pick and from the
    /// section-lifecycle seed.
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

    /// Serialize a bulk-state wipe against a data write. Held only around the write; never across
    /// an `.await`.
    pub(super) fn gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.section.gate()
    }

    /// Forget the collage recorded as on screen, so the next refresh recomposes. Paired with the
    /// leave handler's hero-slot wipe rather than with [`Self::release_section_state`] — see
    /// [`crate::ui::favorites::FavoritesUi::forget_mosaic`].
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.forget();
    }

    /// Drop every section-local buffer so a hidden page holds nothing. Runs off the UI thread on
    /// section leave, which set `mark_dirty()` and released the grid synchronously. Release order
    /// matches `FavoritesUi::release_section_state`, gate included, so no fetch can land halfway
    /// through the wipe — and each fetcher's own `section_active()` bail, plus the grid's revoke
    /// on its release, is what stops one resolving *after* the leave from repopulating it.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        crate::ui::grid_prewarm::shrink_covers();
        {
            let _gate = self.gate();
            self.inner.tracks_all.lock().clear();
            // The folds go with the sets they summarise: a derived value outliving its source is
            // the one thing the band can state that is *wrong* rather than merely absent.
            *self.inner.songs_totals.lock() = SongsTotals::default();
            *self.inner.songs_fold.lock() = HeroFold::default();
            *self.inner.most_played_totals.lock() = MostPlayedTotals::default();
        }
        // Re-armed where the wipe is rather than left to the caller: the grid is empty now, and
        // relying on the leave's own `mark_dirty` is a coupling two files apart that a second
        // caller of this function would not know to honour.
        self.mark_grid_dirty();
        melodia_platform::services::platform::allocator::trim();
    }

    pub(crate) fn state(&self) -> &RecentlyPlayedUiState {
        &self.inner
    }

    /// Track ids of the post-filter Songs tab in **display order** — recency, less whatever the
    /// search bar has narrowed away — so `shuffle-all` / `play-row` enqueue what the user sees.
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

    /// Flip `is_favorite` on a cached row so a single-row toggle lands without a
    /// re-fetch. Unlike Favorites it never removes the row — recency membership
    /// is independent of both flags.
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

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<RecentlyPlayedUi>();
};

#[cfg(test)]
#[path = "tests/recently_played_tests.rs"]
mod tests;
