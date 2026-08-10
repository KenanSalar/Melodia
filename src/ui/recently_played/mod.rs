//! Recently-Played view glue between Rust and Slint.
//!
//! Drives the `RecentlyPlayed` Slint global (sidebar index 8) — the shared
//! `MosaicTabHero` banner over two mutually exclusive sub-views:
//!
//! * Hero — a live 2×2 mosaic of the up-to-4 most-recently-played distinct
//!   covers behind the count, the running time and the per-tab action pill,
//!   under a header row carrying the tab bar and the filter.
//! * Songs — a `TrackList` of the 200 most-recently-played tracks
//!   (`last_played DESC`), bound to the post-filter `RecentlyPlayed.tracks`
//!   model. The set is fetched once per refresh; keystrokes re-walk the cached
//!   `tracks_all` **in memory** (membership is fixed to the 200). The list is
//!   mounted `sortable: false` — recency is the point of the page, so its
//!   column headers resize and toggle but never re-order.
//! * Most Played — a virtualized `EntityCardGrid` over the library's played
//!   tracks, ranked by count, from an uncapped fetch.
//!
//! Cache discipline mirrors `src/ui/favorites`: per-tier `CoverThumbs` LRUs (the
//! shared row tier plus dedicated mosaic and grid-sized tiers), released on
//! section leave — and, for the grid, on tab-leave — so the hidden view's
//! resident footprint drops to ~0. Re-enter re-fetches via the
//! `library_changed_tx` / `stats_changed_tx`-driven `mark_dirty` / `take_dirty`
//! round-trip.
//!
//! The tree is split by the question each file answers rather than by tab. This
//! file is the handle and its teardown; `tabs.rs` is the sub-view enum and its
//! seeding, `covers.rs` the three cover tiers, `rows.rs` the Slint models,
//! `hero.rs` the band, `songs.rs` the Songs tab, and `grid/` the Most Played tab
//! in three parts — `fetch` (the query), `apply` (cache → model) and `warm` (the
//! one cache-coherence predicate that names these tabs).

mod callbacks;
mod covers;
mod grid;
mod hero;
mod rows;
mod selection;
mod songs;
mod state;
mod tabs;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::hero_folds::{HeroFold, MostPlayedTotals};
use crate::ui::section_state::SectionState;
use crate::ui::view_ctx::ViewCtx;

use state::{
    GRID_THUMB_CAP, MOSAIC_THUMB_CAP, MOSAIC_THUMB_SIZE, MOST_PLAYED_THUMB_SIZE,
    RecentlyPlayedUiState, SongsTotals,
};

/// This page's `Nav.selected-index` — see [`crate::ui::favorites::NAV_FAVORITES`].
pub const NAV_RECENTLY_PLAYED: i32 = 8;

// `boot::ui_setup` retunes the cover cap once the window is live.
pub use covers::tune_cache_for_display;

// Reached only from this slice's own `callbacks/`, which used to live two
// modules away, plus `ui::hero_chips` for the tab enum and the nav index.
// `pub(super)` is `pub(in crate::ui)` here, which is exactly that reach.
pub(super) use grid::{apply_filtered_grid_now, mark_covers_warm, refresh_grid};
pub(super) use rows::to_slint_most_played_row;
pub(super) use selection::{clear_selection, handle_select_row};
pub(super) use songs::{
    apply_filtered_tracks, apply_filtered_tracks_now, apply_row_favorite, apply_row_rating,
    refresh_tracks, set_filter,
};
pub(super) use tabs::{seed_tab, tab_from_index};
pub use tabs::RecentlyPlayedTab;

/// Install the Recently-Played models, build the handle, wire every
/// `RecentlyPlayed.*` callback, and seed the persisted tab.
///
/// A trimmed Favorites: the shared row-tier `cover_thumbs` serves the Songs
/// list, while the handle allocates its own mosaic and Most Played LRUs (the
/// latter released on tab-leave as well as on section-leave). Its row-menu
/// "Go to …" entries are wired centrally by `wire_cross_tab_nav`, so it takes
/// no peer handle.
///
/// The tab seed folds in here for the reason it does on Favorites — see
/// [`crate::ui::favorites::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<RecentlyPlayedUi> {
    rows::install_recently_played_models(cx.app);
    let rp_ui = Arc::new(RecentlyPlayedUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, &rp_ui);
    if let Some(vs) = cx.view_state {
        seed_tab(cx.app, &rp_ui, vs.recently_played_tab);
    }
    rp_ui
}

/// Rust-side state for the Recently-Played view. Shared between the UI
/// callbacks (`callbacks::wire`) and the async fetchers behind an
/// `Arc<RecentlyPlayedUi>` — `Send + Sync`.
pub struct RecentlyPlayedUi {
    inner: RecentlyPlayedUiState,
    /// Shared row-tier (72 px) cache — used for the Songs tab's `TrackList` row
    /// column. Same instance every view consumes.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Hero mosaic-tile cache (128 px). Released on section leave.
    pub(super) mosaic_thumbs: Arc<CoverThumbs>,
    /// Most Played grid cache. Released on section leave and on tab-leave.
    pub(super) most_played_thumbs: Arc<CoverThumbs>,
    /// Visibility + staleness + the mutation gate, the same unit the four
    /// entity grids carry. `active` mirrors `Nav.selected-index ==
    /// NAV_RECENTLY_PLAYED && !Nav.now-playing-open`, and gates the refresh
    /// subscriber so a background tick doesn't repaint a hidden view.
    section: SectionState,
    /// Synchronous shadow of `RecentlyPlayed.tab-idx`, as a
    /// [`RecentlyPlayedTab`]. The off-thread fetchers decide which model to fill
    /// and which cover tier to warm from this — only one sub-view is ever
    /// mounted, so doing both is twice the work and twice the resident buffers
    /// for a surface nobody can scroll.
    active_tab: AtomicU8,
    /// [`SectionState`]'s dirty flag one level down, for the Most Played tab
    /// alone.
    ///
    /// **The section flag can't answer this, because the page has one
    /// `SectionActiveGate` where My Library has one per tab.** Both fetches used
    /// to run on every `stats_changed` tick — one per finished track — so the
    /// Songs tab paid a full library-wide `get_most_played`, a fold over every
    /// played row and a store of the lot, for a grid the user cannot see and may
    /// never open. Gating the fetch on the mounted tab is what makes that
    /// nothing; this is where the tick it skipped is remembered, so the pick
    /// that mounts the grid knows to fetch. Seeded `true`, the first pick
    /// therefore always fetching.
    grid_dirty: AtomicBool,
}

impl RecentlyPlayedUi {
    fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            inner: RecentlyPlayedUiState::new(),
            cover_thumbs,
            mosaic_thumbs: Arc::new(CoverThumbs::with_config(
                MOSAIC_THUMB_SIZE,
                MOSAIC_THUMB_CAP,
            )),
            most_played_thumbs: Arc::new(CoverThumbs::with_config(
                MOST_PLAYED_THUMB_SIZE,
                GRID_THUMB_CAP,
            )),
            section: SectionState::new(),
            active_tab: AtomicU8::new(RecentlyPlayedTab::Songs.as_code()),
            grid_dirty: AtomicBool::new(true),
        }
    }

    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Mirror the mounted sub-view. Written on the UI thread from the tab bar's
    /// pick and from the section-lifecycle seed.
    pub fn set_active_tab(&self, tab: RecentlyPlayedTab) {
        self.active_tab.store(tab.as_code(), Ordering::Relaxed);
    }

    /// Which sub-view is mounted right now.
    pub fn active_tab(&self) -> RecentlyPlayedTab {
        RecentlyPlayedTab::from_code(self.active_tab.load(Ordering::Relaxed))
    }

    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag.
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
    }

    /// Remember that a refresh tick reached the page while the Most Played tab
    /// was not the one mounted. See [`Self::grid_dirty`].
    pub fn mark_grid_dirty(&self) {
        self.grid_dirty.store(true, Ordering::Release);
    }

    /// Atomically read-and-clear it — `true` iff the Most Played cache has
    /// missed a tick since the last fetch.
    pub fn take_grid_dirty(&self) -> bool {
        self.grid_dirty.swap(false, Ordering::AcqRel)
    }

    /// Serialize a bulk-state wipe against a data write. Held only around
    /// the write; never across an `.await`.
    pub(super) fn gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.section.gate()
    }

    /// Forget the mosaic recorded as being on screen, so the next refresh
    /// recomposes the hero blur. Paired with the leave handler's `blur-img-*`
    /// wipe rather than with [`Self::release_section_state`] — see
    /// [`crate::ui::favorites::FavoritesUi::forget_mosaic`], including for why
    /// this is the guard's only mover outside `hero.rs`.
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.lock().clear();
    }

    /// Forget what the grid last painted, so the next apply rebuilds instead of
    /// recognising its own output and skipping.
    ///
    /// Sits beside [`Self::forget_mosaic`] at the section-leave call site, and is
    /// unconditional for the same reason: the model is emptied there, so a
    /// signature that survived would match the identical data on re-enter and
    /// leave the grid blank.
    pub fn forget_grid_signature(&self) {
        self.inner.last_grid_signature.lock().take();
    }

    /// Drop every section-local resident buffer so the hidden view's
    /// footprint drops to ~0. Called (off the UI thread) on section leave;
    /// `mark_dirty()` was set synchronously on the same leave so the
    /// section-enter handler re-fetches via `take_dirty()`. Release ordering
    /// matches `FavoritesUi::release_section_state`, gate included: the state
    /// wipe is serialized against a `refresh_grid` / `refresh_tracks` store so
    /// neither can land halfway through the other.
    ///
    /// The gate serializes those writes; it does **not** order them, so it is
    /// not what stops a fetch that resolves *after* the leave from repopulating
    /// what this just emptied. That is each fetcher's own `section_active()`
    /// bail, plus the one inside each apply's event-loop closure — the pair
    /// `favorites::grids` carries, and the reason the wipe can be unconditional
    /// below the early return.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.mosaic_thumbs.clear();
        self.most_played_thumbs.clear();
        {
            let _gate = self.gate();
            self.inner.tracks_all.lock().clear();
            self.inner.most_played.lock().clear();
            // The folds go with the caches they summarise: a derived value that
            // outlives its source is the one thing the band can state that is
            // *wrong* rather than merely absent. The two fetches run
            // concurrently, so on a re-enter whichever lands first would
            // otherwise pair a fresh count with a pre-leave spread.
            *self.inner.songs_totals.lock() = SongsTotals::default();
            *self.inner.songs_fold.lock() = HeroFold::default();
            *self.inner.most_played_totals.lock() = MostPlayedTotals::default();
            self.inner.applied_selection.lock().clear();
        }
        // Stated where the wipe is rather than left to the caller: this empties the
        // very cache the flag guards, and the leave's own `mark_dirty` re-arming it
        // through `kick_full_refresh` is a coupling two files apart that a fifth
        // caller of this function would not know to honour.
        self.mark_grid_dirty();
        crate::tasks::heap_trim::trim();
    }

    pub(crate) fn state(&self) -> &RecentlyPlayedUiState {
        &self.inner
    }

    /// Track ids of the Most Played grid, in card order. `play-track` and
    /// `shuffle-most-played` hand these to `player_play_tracks` so a card loads
    /// that grid rather than the recency list on the other tab.
    ///
    /// Filtered through the same predicate `grid::apply::build_filtered_grid`
    /// builds the model with — the grid narrows with the hero search bar, so the
    /// raw cache would enqueue cards that aren't on screen.
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

    /// Track ids of the post-filter Songs tab in **display order** — recency,
    /// less whatever the search bar has narrowed away — so `shuffle-all` /
    /// `play-row` enqueue what the user sees.
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

    /// Surgically flip `is_favorite` on a cached row so a single-row toggle
    /// reflects in the model without a full re-fetch. Unlike Favorites this
    /// never removes the row — recency membership is independent of the
    /// favorite flag.
    pub fn flip_track_favorite(&self, id: i64, fav: bool) {
        if let Some(r) = self.inner.tracks_all.lock().iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
    }

    /// Surgically set `rating` on a cached row — the star-rating analogue of
    /// [`Self::flip_track_favorite`]. Recency membership is fixed to the 200
    /// most-recent rows, so rating never removes the row.
    pub fn flip_track_rating(&self, id: i64, rating: i32) {
        if let Some(r) = self.inner.tracks_all.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<RecentlyPlayedUi>();
};

#[cfg(test)]
#[path = "tests/recently_played_tests.rs"]
mod tests;
