//! The Favorites page: a pinned hero over three mutually exclusive tabs.
//!
//! The hero's mosaic tiles are the first 4 *distinct* covers of the Most Played tab's own walk —
//! both rank through one clause in the store's ranking queries rather than two hand-matched
//! ones, so they can't disagree about a tie — topped up from the favourites that tab
//! excludes once the played covers run out. Songs binds a `TrackList` over a
//! [`TrackRows`](crate::ui::paged_rows::track_rows::TrackRows) whose search *and* sort run in SQL;
//! Most Played and Favorite Artists are paged grids whose search and sort do too. There is no
//! Favorite Albums tab, the Albums page covering that.
//!
//! The page's shared contract — counts, dirty flags, cover generations — is
//! `.claude/rules/ui-patterns.md`'s "three tabbed pages" block; the deltas below are this page's.
//!
//! The tree is split by the question each file answers rather than by tab. This file is the handle
//! and its teardown; `tabs.rs` the sub-view enum and its seeding, `rows.rs` the Slint models,
//! `hero.rs` the band, `songs.rs` the Songs tab, and `grids/` the two grid tabs.

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

/// This page's `Nav.selected-index`. **The single definition**, beside the view it names, the way
/// [`crate::ui::my_library::NAV_MY_LIBRARY`] sits beside its page — the cross-tab hand-off stamps
/// it as an origin, the lifecycle seeds the section-active shadow from it, and `hero_chips` asks
/// which band is up.
pub const NAV_FAVORITES: i32 = 2;

// `pub(super)` is `pub(in crate::ui)` here, exactly the reach these need: this slice's own
// `callbacks/`, plus `ui::hero_chips`.
pub(super) use grids::{prewarm_tab_covers, release_grids, reload_grid, requery_grid, set_columns};
pub(super) use hero::refresh_hero;
pub(super) use rows::{most_played_ids, to_slint_fav_artist_row};
pub(super) use songs::{
    current_filter, displayed_ids, release_tracks, requery_tracks, set_filter, set_sort,
};
pub use tabs::FavoritesTab;
pub(super) use tabs::{seed_tab, tab_from_index};

/// Install the Favorites models, build the handle, wire every `Favorites.*` callback, and seed the
/// persisted tab.
///
/// `artists_ui` is the cross-tab open-artist hand-off, taken as a parameter so the ordering is a
/// compile error rather than a comment.
///
/// **The tab seed folds in here** rather than sitting with its siblings in
/// `hydrate_ui_from_settings`, because seeding writes the Slint property *and* the handle's
/// synchronous shadow, which the off-thread fetchers read to decide which model to fill and which
/// tier to warm.
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

/// Rust-side state for the Favorites view, shared between the UI callbacks and the async fetchers
/// behind an `Arc`.
pub struct FavoritesUi {
    inner: FavoritesUiState,
    /// The shared row tier, for the Songs tab's `TrackList` column.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The band's blur shape, or `None` under the aurora setting — read once at install, the
    /// setting being restart-gated, because the compose runs where no window handle is in reach.
    pub(super) hero_blur: Option<BlurSpec>,
    /// The global the Songs list lands through.
    global: Weak<Favorites<'static>>,
    /// What the Songs tab's loads share, and each grid tab's.
    songs: ListLoads,
    pub(super) most_played: GridLoads,
    pub(super) artists: GridLoads,
    /// Visibility + staleness + the mutation gate, the unit every entity grid carries.
    section: SectionState,
    /// Synchronous shadow of `Favorites.tab-idx`. The off-thread loads read which tab is mounted
    /// from it — only one is, so loading or warming the others is work no frame draws.
    active_tab: AtomicU8,
    /// [`SectionState`]'s dirty flag one level down, per tab — the page has one
    /// `SectionActiveGate` for three mutually exclusive tabs, so without these a refresh tick
    /// ran all three loads whatever was mounted.
    ///
    /// Gating is safe here only because `hero_chips::favorites_chips` is a per-tab match and
    /// `refresh_hero` — which answers the count, running time and mosaic on every tab — is never
    /// gated. All seeded `true`, so the first pick onto a tab always loads.
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

    /// Mirror the mounted sub-view. Written on the UI thread, from the tab bar's pick and from the
    /// section-lifecycle seed.
    pub fn set_active_tab(&self, tab: FavoritesTab) {
        self.active_tab.store(tab.as_code(), Ordering::Relaxed);
    }

    pub fn active_tab(&self) -> FavoritesTab {
        FavoritesTab::from_code(self.active_tab.load(Ordering::Relaxed))
    }

    /// Whether the mounted tab shows a filtered list, the ids a tab pick clearing the shared
    /// needle leaves out of date.
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

    /// Remember that a refresh tick reached the page with Songs not mounted.
    pub fn mark_songs_dirty(&self) {
        self.mark_tab_dirty(FavoritesTab::Songs);
    }

    /// Serialize a bulk-state wipe against a data write. Held only around the write; never across
    /// an `.await`.
    pub(super) fn gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.section.gate()
    }

    /// Forget the collage recorded as on screen, so the next refresh recomposes instead of
    /// skipping on an unchanged cover set.
    ///
    /// The section leave is the only caller, and it sits beside the hero-slot wipe rather than in
    /// [`Self::release_section_state`] — that one bails when the user has already come back, where
    /// the wipe is unconditional and would leave the guard set against a hero with no artwork left
    /// to guard.
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.forget();
    }

    /// Drop every section-local buffer, so a hidden page holds nothing. Runs off the UI thread on
    /// section leave, which set `mark_dirty()` and released the lists and grids synchronously.
    ///
    /// Per-section LRUs go first, then the state under the section gate, then one
    /// `allocator::trim` over all of it.
    ///
    /// The gate serializes those stores but does **not** order them, so it is not what stops a
    /// fetch resolving *after* the leave from repopulating what this emptied. That is each
    /// fetcher's own `section_active()` bail, and for the grids the revoke on their release.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        crate::ui::grid_prewarm::shrink_covers();
        {
            let _gate = self.gate();
            *self.inner.stats.lock() = FavoriteStats::default();
            // The totals go with the grids they summarise: a derived value outliving its source
            // is the one thing the band can state that is *wrong* rather than merely absent.
            *self.inner.most_played_totals.lock() = MostPlayedTotals::default();
            *self.inner.artist_total.lock() = 0;
        }
        // Re-armed beside the state they guard rather than left to the leave's own `mark_dirty`
        // two files away: every list and grid is empty now (the leave empties them on the UI
        // thread), so the next tab entered owes a load whatever the flags held on the way out.
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

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<FavoritesUi>();
};

#[cfg(test)]
#[path = "tests/favorites_tests.rs"]
mod tests;
