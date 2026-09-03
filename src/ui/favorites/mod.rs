//! The Favorites page: a pinned hero over three mutually exclusive tabs.
//!
//! The hero's mosaic tiles are the first 4 *distinct* covers of the Most Played tab's own walk —
//! both rank through one shared clause (`queries::track::MOST_PLAYED_ORDER`) rather than two
//! hand-matched ones, so they can't disagree about a tie — topped up from the favourites that tab
//! excludes once the played covers run out. Songs binds a `TrackList` whose search *and* sort run
//! in memory off [`crate::ui::track_list_cache`]; Most Played and Favorite Artists are virtualized
//! grids over uncapped fetches. There is no Favorite Albums tab, the Albums page covering that.
//!
//! The page's shared contract — counts, dirty flags, signature skips, cover generations — is
//! `.claude/rules/ui-patterns.md`'s "three tabbed pages" block; the deltas below are this page's.
//!
//! The tree is split by the question each file answers rather than by tab. This file is the handle
//! and its teardown; `tabs.rs` the sub-view enum and its seeding, `covers.rs` the three tiers,
//! `rows.rs` the Slint models, `hero.rs` the band, `songs.rs` the Songs tab, and `grids/` the two
//! grid tabs in four parts — `fetch`, `apply`, `warm` and `sort`.

mod callbacks;
mod covers;
mod grids;
mod hero;
mod rows;
mod selection;
mod songs;
mod state;
mod tabs;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::entities::track::FavoriteStats;
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::ui::artists::ArtistsUi;
use crate::ui::artwork_cache::BlurSpec;
use crate::ui::detail_artwork;
use crate::ui::hero_folds::{HeroFold, MostPlayedTotals};
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::view_ctx::ViewCtx;

use state::{FavoritesUiState, GRID_THUMB_CAP};

/// This page's `Nav.selected-index`. **The single definition**, beside the view it names, the way
/// [`crate::ui::my_library::NAV_MY_LIBRARY`] sits beside its page — the cross-tab hand-off stamps
/// it as an origin, the lifecycle seeds the section-active shadow from it, and `hero_chips` asks
/// which band is up.
pub const NAV_FAVORITES: i32 = 2;

// `boot::ui_setup` retunes the cover cap once the window is live.
pub use covers::tune_cache_for_display;

// `pub(super)` is `pub(in crate::ui)` here, exactly the reach these need: this slice's own
// `callbacks/`, plus `ui::hero_chips`.
pub(super) use grids::{
    apply_filtered_grids_now, mark_covers_warm, refresh_grids, set_artist_sort,
};
pub(super) use hero::refresh_hero;
pub(super) use rows::{to_slint_fav_artist_row, to_slint_most_played_row};
pub(super) use selection::{clear_selection, handle_select_row};
pub(super) use songs::{
    apply_filtered_tracks, apply_filtered_tracks_now, apply_row_rating, refresh_tracks,
    resort_and_apply, set_filter, set_sort,
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
    rows::install_favorites_models(cx.app);
    let favorites_ui =
        Arc::new(FavoritesUi::new(cx.cover_thumbs.clone(), detail_artwork::blur_spec(cx.app)));
    callbacks::wire(cx.app, cx.state, cx.view_state, &favorites_ui, artists_ui);
    for tier in [
        &favorites_ui.most_played_thumbs,
        &favorites_ui.artist_thumbs,
    ] {
        crate::ui::cover_generation::notify_on_decode(tier, cx.app, grids::repaint_covers);
    }
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
    /// The two grid tiers, released on section leave *and* on tab-leave.
    pub(super) most_played_thumbs: Arc<CoverThumbs>,
    pub(super) artist_thumbs: Arc<CoverThumbs>,
    /// Visibility + staleness + the mutation gate, the unit every entity grid carries.
    section: SectionState,
    /// Synchronous shadow of `Favorites.tab-idx`. The off-thread fetchers pick which cover tier to
    /// warm from it — only one grid is ever mounted, so warming both is twice the resident buffers
    /// for nothing.
    active_tab: AtomicU8,
    /// [`SectionState`]'s dirty flag one level down, per fetch rather than per tab — the page has
    /// one `SectionActiveGate` for three mutually exclusive tabs, so without these a refresh tick
    /// ran all three fetches whatever was mounted. Two flags rather than three because one
    /// `refresh_grids` feeds both grid tabs.
    ///
    /// Gating is safe here only because `hero_chips::favorites_chips` is a per-tab match and
    /// `refresh_hero` — which answers the count, running time and mosaic on every tab — is never
    /// gated. Both seeded `true`, so the first pick onto a tab always fetches.
    songs_dirty: AtomicBool,
    grids_dirty: AtomicBool,
}

impl FavoritesUi {
    fn new(cover_thumbs: Arc<CoverThumbs>, hero_blur: Option<BlurSpec>) -> Self {
        Self {
            inner: FavoritesUiState::new(),
            cover_thumbs,
            hero_blur,
            most_played_thumbs: Arc::new(CoverThumbs::with_config(
                crate::ui::grid_prewarm::GRID_COVER_FALLBACK,
                GRID_THUMB_CAP,
            )),
            // Same tier as Most Played — the circular mask is applied at draw time, so the source
            // needs no extra resolution.
            artist_thumbs: Arc::new(CoverThumbs::with_config(
                crate::ui::grid_prewarm::GRID_COVER_FALLBACK,
                GRID_THUMB_CAP,
            )),
            section: SectionState::new(),
            active_tab: AtomicU8::new(FavoritesTab::Songs.as_code()),
            songs_dirty: AtomicBool::new(true),
            grids_dirty: AtomicBool::new(true),
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

    /// Remember that a refresh tick reached the page with Songs not mounted.
    pub fn mark_songs_dirty(&self) {
        self.songs_dirty.store(true, Ordering::Release);
    }

    /// `true` iff the Songs cache has missed a tick since its last fetch.
    pub fn take_songs_dirty(&self) -> bool {
        self.songs_dirty.swap(false, Ordering::AcqRel)
    }

    /// The grid tabs' equivalent — one flag, because one fetch fills both.
    pub fn mark_grids_dirty(&self) {
        self.grids_dirty.store(true, Ordering::Release);
    }

    pub fn take_grids_dirty(&self) -> bool {
        self.grids_dirty.swap(false, Ordering::AcqRel)
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

    /// Forget what the grids last painted, so the next apply rebuilds instead of recognising its
    /// own output and skipping.
    ///
    /// Sits beside [`Self::forget_mosaic`] at the section-leave call site, and is unconditional for
    /// the same reason: the models are emptied there, so a signature that survived would match the
    /// identical data on re-enter and leave the grid blank.
    pub fn forget_grid_signature(&self) {
        self.inner.last_grid_signature.lock().take();
    }

    /// Drop every section-local buffer and clear the Slint models, so a hidden page holds nothing.
    /// Runs off the UI thread on section leave, which set `mark_dirty()` synchronously.
    ///
    /// Release order matches `AlbumsUi::release_section_state`: per-section LRUs first, then
    /// mutated state under the section gate, then an `allocator::trim`.
    ///
    /// The gate serializes those stores but does **not** order them, so it is not what stops a
    /// fetch resolving *after* the leave from repopulating what this emptied. That is each
    /// fetcher's own `section_active()` bail — one after the query, one after the prewarm's
    /// `.await` — plus the one inside each apply's event-loop closure.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.most_played_thumbs.clear();
        self.artist_thumbs.clear();
        {
            let _gate = self.gate();
            self.inner.tracks_all.clear();
            *self.inner.stats.lock() = FavoriteStats {
                count: 0,
                total_duration_ms: 0,
                artwork_paths: Vec::new(),
            };
            self.inner.most_played.lock().clear();
            self.inner.fav_artists.lock().clear();
            // The folds go with the caches they summarise: a derived value outliving its source is
            // the one thing the band can state that is *wrong* rather than merely absent.
            *self.inner.songs_fold.lock() = HeroFold::default();
            *self.inner.most_played_totals.lock() = MostPlayedTotals::default();
            self.inner.applied_selection.lock().clear();
        }
        // Re-armed beside the caches they guard rather than left to the leave's own `mark_dirty`
        // two files away: all three sets are empty now, so the next tab entered owes a fetch
        // whatever the flags held on the way out.
        self.mark_songs_dirty();
        self.mark_grids_dirty();
        crate::services::platform::allocator::trim();
    }

    pub(crate) fn state(&self) -> &FavoritesUiState {
        &self.inner
    }

    /// Track ids of the post-filter Songs tab, in display order — how `shuffle-all` and `play-row`
    /// recover ids without touching the Slint model, callers being off the UI thread.
    pub fn filtered_track_ids(&self) -> Vec<i64> {
        let needle = self.inner.filter.lock().clone();
        self.inner.tracks_all.snapshot().ids_filtered(&needle)
    }

    /// Track ids of the Most Played grid, in card order, so a card there loads that grid rather
    /// than the Songs tab's list. Filtered through the same predicate
    /// `grids::apply::build_filtered_grids` builds the model with — the grid narrows with the
    /// search bar, and the raw cache would enqueue cards that aren't on screen.
    pub fn most_played_track_ids(&self) -> Vec<i64> {
        let needle = self.inner.filter.lock().clone();
        self.inner
            .most_played
            .lock()
            .iter()
            .filter(|t| crate::ui::row_match::most_played_matches(t, &needle))
            .map(|t| t.id)
            .collect()
    }

    /// Flip `is_favorite` on a cached Songs-tab row so a single-row toggle lands without a
    /// re-fetch. `false` removes the row outright, for parity with a fetch that only ever returns
    /// `is_favorite = TRUE`.
    pub fn flip_or_remove_track(&self, id: i64, fav: bool) {
        if fav {
            self.inner.tracks_all.set_favorite(id, true);
        } else {
            self.inner.tracks_all.remove(id);
        }
    }

    /// Set `rating` on a cached row. Unlike [`Self::flip_or_remove_track`], rating never affects
    /// membership, so the row is only patched.
    pub fn flip_track_rating(&self, id: i64, rating: i32) {
        self.inner.tracks_all.set_rating(id, rating);
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
