//! Favorites-view glue between Rust and Slint.
//!
//! Drives the `Favorites` Slint global (sidebar index 2) — a pinned hero
//! over three mutually exclusive sub-views:
//!
//! * Hero — live 2×2 cover mosaic + count + total duration + the per-tab
//!   action pill, under a header row carrying the tab bar and the filter.
//!   The mosaic refreshes whenever `library_changed_tx` ticks (favourite
//!   toggled, play count bumped, library scanned). Its tiles are the first 4
//!   *distinct* covers of the Most Played tab's own walk — the two rank with
//!   one shared clause (`queries::track::MOST_PLAYED_ORDER`) rather than two
//!   hand-matched ones, so they can't disagree about a tie — topped up from the
//!   favourites that tab excludes once the played covers run out. The blur
//!   backdrop fades through the shared `Favorites.blur-img-{a,b}` dual-slot
//!   pattern.
//! * Songs — a full `TrackList` bound to the post-filter
//!   `Favorites.tracks` model. Both the search and the sort are in-memory
//!   off [`crate::ui::track_list_cache`]: the SQL fetch returns the entire
//!   set once per `library_changed_tx` tick, then a keystroke re-walks the
//!   cached `tracks_all` and a header click re-permutes it, neither
//!   hitting `SQLite`.
//! * Most Played and Favorite Artists — virtualized `EntityCardGrid`s over
//!   uncapped fetches from `library::favorites::*`. (No Favorite Albums
//!   tab; the Albums tab covers that.)
//!
//! Cache discipline mirrors `src/ui/albums`: per-tier `CoverThumbs` LRUs
//! (the shared row tier plus dedicated grid-sized tiers for the mosaic /
//! Most Played / Artists), released on Favorites-section leave — and, for
//! the two grids, on tab-leave, which is the event the collapse toggles
//! used to be — so the hidden view's resident footprint drops to ~0.
//! Re-enter re-fetches via `library_changed_tx`-driven `mark_dirty` /
//! `take_dirty` round-trip.
//!
//! The tree is split by the question each file answers rather than by tab.
//! This file is the handle and its teardown; `tabs.rs` is the sub-view enum and
//! its seeding, `covers.rs` the four cover tiers, `rows.rs` the Slint models,
//! `hero.rs` the band, `songs.rs` the Songs tab, and `grids/` the two grid tabs
//! in four parts — `fetch` (queries), `apply` (caches → models), `warm` (the
//! three cache-coherence predicates) and `sort`.

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
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::artists::ArtistsUi;
use crate::ui::hero_folds::{HeroFold, MostPlayedTotals};
use crate::ui::section_state::SectionState;
use crate::ui::view_ctx::ViewCtx;

use state::{FavoritesUiState, GRID_THUMB_CAP, MOSAIC_THUMB_CAP, MOSAIC_THUMB_SIZE};

/// This page's `Nav.selected-index`. **The single definition**, beside the view it
/// names, the way [`crate::ui::my_library::NAV_MY_LIBRARY`] sits beside its page —
/// the cross-tab hand-off stamps it as an origin, the lifecycle seeds the
/// section-active shadow from it, and `hero_chips` asks which band is up.
pub const NAV_FAVORITES: i32 = 2;

// `boot::ui_setup` retunes the cover cap once the window is live.
pub use covers::tune_cache_for_display;

// Reached only from this slice's own `callbacks/`, which used to live two
// modules away, plus `ui::hero_chips` for the tab enum and the nav index.
// `pub(super)` is `pub(in crate::ui)` here, which is exactly that reach.
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

/// Install the Favorites models, build the handle, wire every `Favorites.*`
/// callback, and seed the persisted tab.
///
/// Takes `artists_ui` because the sub-view module borrows it for the cross-tab
/// open-artist hand-off — which is the ordering, made a compile error rather
/// than a comment: this does not resolve until `artists_ui` is bound.
///
/// **The tab seed folds in here** rather than sitting with its siblings in
/// `hydrate_ui_from_settings`, and the handle is why: seeding writes the Slint
/// property *and* the handle's synchronous shadow, which the off-thread
/// fetchers read to decide which model to fill and which cover tier to warm.
/// Folded, the handle is the receiver instead of something that has to still be
/// in scope two functions later.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>, artists_ui: &Arc<ArtistsUi>) -> Arc<FavoritesUi> {
    rows::install_favorites_models(cx.app);
    let favorites_ui = Arc::new(FavoritesUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, &favorites_ui, artists_ui);
    if let Some(vs) = cx.view_state {
        seed_tab(cx.app, &favorites_ui, vs.favorites_tab);
    }
    favorites_ui
}

/// Rust-side state for the Favorites view. Shared between the UI
/// callbacks (`callbacks::wire`) and the async fetchers behind an
/// `Arc<FavoritesUi>` — `Send + Sync`.
pub struct FavoritesUi {
    inner: FavoritesUiState,
    /// Shared row-tier cache — used for the Songs tab's
    /// `TrackList` row column. Same instance every view consumes.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Mosaic-tile cache (128 px). Released on section leave; warm
    /// across mosaic refreshes inside one section visit.
    pub(super) mosaic_thumbs: Arc<CoverThumbs>,
    /// Most Played grid cache. Released on section leave and on tab-leave.
    pub(super) most_played_thumbs: Arc<CoverThumbs>,
    /// Favorite Artists grid cache (circular cards). Released on section
    /// leave and on tab-leave.
    pub(super) artist_thumbs: Arc<CoverThumbs>,
    /// Visibility + staleness + the mutation gate, the same unit the four
    /// entity grids carry. `active` mirrors
    /// `Nav.selected-index == NAV_FAVORITES && !Nav.now-playing-open`.
    section: SectionState,
    /// Synchronous shadow of `Favorites.tab-idx`, as a [`FavoritesTab`].
    /// The off-thread fetchers decide which cover tier to warm from this —
    /// only one grid is ever mounted, so warming both is half the decodes
    /// and twice the resident buffers for nothing.
    active_tab: AtomicU8,
    /// [`SectionState`]'s dirty flag one level down, per fetch rather than per
    /// tab — the `RecentlyPlayedUi::grid_dirty` shape, one page over.
    ///
    /// **The page has one `SectionActiveGate` for three mutually exclusive
    /// tabs**, so without these `kick_full_refresh` ran all three fetches on
    /// every tick regardless of what was mounted. Everything downstream already
    /// knew better — `build_filtered_tracks` returns `None` off Songs,
    /// `build_filtered_grids` materialises only the mounted tab — so with a grid
    /// tab up, `refresh_tracks` was still fetching every favourite, sorting it
    /// and converting the lot into rows that reached nothing, once per finished
    /// track. Two flags rather than three because `refresh_grids` is one fetch
    /// feeding both grid tabs.
    ///
    /// What makes gating safe here is that `hero_chips::favorites_chips` is a
    /// per-tab match: each tab's chips come from the fetch that tab needs, and
    /// `refresh_hero` — which answers the count, the running time and the mosaic
    /// on every tab — is never gated.
    ///
    /// Both seeded `true`, so the first pick onto a tab always fetches.
    songs_dirty: AtomicBool,
    grids_dirty: AtomicBool,
}

impl FavoritesUi {
    fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            inner: FavoritesUiState::new(),
            cover_thumbs,
            mosaic_thumbs: Arc::new(CoverThumbs::with_config(MOSAIC_THUMB_SIZE, MOSAIC_THUMB_CAP)),
            most_played_thumbs: Arc::new(CoverThumbs::with_config(
                crate::ui::grid_prewarm::GRID_COVER_SIZE,
                GRID_THUMB_CAP,
            )),
            // Same tier as Most Played — the circular mask is applied at draw
            // time, so the source needs no extra resolution.
            artist_thumbs: Arc::new(CoverThumbs::with_config(
                crate::ui::grid_prewarm::GRID_COVER_SIZE,
                GRID_THUMB_CAP,
            )),
            section: SectionState::new(),
            active_tab: AtomicU8::new(FavoritesTab::Songs.as_code()),
            songs_dirty: AtomicBool::new(true),
            grids_dirty: AtomicBool::new(true),
        }
    }

    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Mirror the mounted sub-view. Written on the UI thread from the tab
    /// bar's pick and from the section-lifecycle seed.
    pub fn set_active_tab(&self, tab: FavoritesTab) {
        self.active_tab.store(tab.as_code(), Ordering::Relaxed);
    }

    /// Which sub-view is mounted right now.
    pub fn active_tab(&self) -> FavoritesTab {
        FavoritesTab::from_code(self.active_tab.load(Ordering::Relaxed))
    }

    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag. The section-enter
    /// handler uses this to decide whether to re-fetch hero + grids +
    /// tracks; the live-refresh subscriber sets it whenever a
    /// `library_changed_tx` tick arrives while the view is hidden.
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
    }

    /// Remember that a refresh tick reached the page while the Songs tab was not
    /// the one mounted. See [`Self::songs_dirty`].
    pub fn mark_songs_dirty(&self) {
        self.songs_dirty.store(true, Ordering::Release);
    }

    /// Atomically read-and-clear it — `true` iff the Songs cache has missed a
    /// tick since the last fetch.
    pub fn take_songs_dirty(&self) -> bool {
        self.songs_dirty.swap(false, Ordering::AcqRel)
    }

    /// The two grid tabs' equivalent. One flag, because one fetch fills both.
    pub fn mark_grids_dirty(&self) {
        self.grids_dirty.store(true, Ordering::Release);
    }

    /// Atomically read-and-clear it.
    pub fn take_grids_dirty(&self) -> bool {
        self.grids_dirty.swap(false, Ordering::AcqRel)
    }

    /// Serialize a bulk-state wipe against a data write. Held only around
    /// the write; never across an `.await`.
    pub(super) fn gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.section.gate()
    }

    /// Forget the mosaic recorded as being on screen, so the next refresh
    /// recomposes the hero blur instead of skipping on an unchanged cover set.
    ///
    /// The section leave is the only caller, and it sits beside the
    /// `blur-img-*` wipe rather than in [`Self::release_section_state`]: that
    /// one bails out when the user has already come back, but the wipe is
    /// unconditional — leaving the guard set against a hero that no longer has
    /// a blur to guard. Every *other* move of the guard is made where the paint
    /// is, inside the `mosaic_hero` apply/clear pair.
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.lock().clear();
    }

    /// Forget what the grids last painted, so the next apply rebuilds instead
    /// of recognising its own output and skipping.
    ///
    /// Sits beside [`Self::forget_mosaic`] at the section-leave call site, and
    /// is unconditional for the same reason: the models are emptied there, so a
    /// signature that survived would match the identical data on re-enter and
    /// leave the grid blank.
    pub fn forget_grid_signature(&self) {
        self.inner.last_grid_signature.lock().take();
    }

    /// Drop every section-local resident buffer + clear the Slint
    /// models so the hidden view's footprint drops to ~0. Called
    /// (off the UI thread) on section leave. `mark_dirty()` was set
    /// synchronously on the same leave so the section-enter handler
    /// re-fetches via `take_dirty()` — no Rust-side cache is preserved
    /// for the re-enter path (a 1000-row favourite library was holding
    /// ~200 KB in `tracks_all` for no benefit, since the in-memory
    /// filter would have been re-walked from a freshly-fetched list
    /// anyway).
    ///
    /// Cache release ordering matches `AlbumsUi::release_section_state`:
    /// per-section LRUs first, then mutated state under the section gate — so
    /// none of the three fetches (`hero::refresh_hero`, `songs::refresh_tracks`,
    /// `grids::fetch::refresh_grids`) can interleave a store with this wipe and
    /// leave half of each on screen — then a `heap_trim::trim` so glibc hands
    /// the freed pages back.
    ///
    /// The gate serializes those stores; it does **not** order them, so it is
    /// not what stops a fetch resolving *after* the leave from repopulating what
    /// this just emptied. That is each fetcher's own `section_active()` bail
    /// (one after the query, one after the cover prewarm's `.await`) plus the
    /// one inside each apply's event-loop closure.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.mosaic_thumbs.clear();
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
            // The folds go with the caches they summarise: a derived value
            // that outlives its source is the one thing the band can state
            // that is *wrong* rather than merely absent. `refresh_hero` is the
            // shortest of the three concurrent fetches, so it publishes first
            // on the re-enter and would otherwise pair a fresh count with a
            // pre-leave spread — "3 favorites · 37 artists" for as long as
            // `refresh_tracks` takes.
            *self.inner.songs_fold.lock() = HeroFold::default();
            *self.inner.most_played_totals.lock() = MostPlayedTotals::default();
            self.inner.applied_selection.lock().clear();
        }
        // Re-armed beside the caches they guard rather than left to the leave's
        // own `mark_dirty` two files away — every one of the three sets above is
        // now empty, so whichever tab is entered next owes a fetch whatever the
        // flags happened to hold when the section went away.
        self.mark_songs_dirty();
        self.mark_grids_dirty();
        crate::tasks::heap_trim::trim();
    }

    pub(crate) fn state(&self) -> &FavoritesUiState {
        &self.inner
    }

    /// Track ids of the post-filter Songs tab, in display order.
    /// `shuffle-all` / `play-row` use this to recover ids without
    /// round-tripping the Slint model.
    pub fn filtered_track_ids(&self) -> Vec<i64> {
        // The post-filter list is the one currently bound to the Slint
        // model; the unfiltered cache is in `tracks_all`. We re-walk
        // the cache + filter here to stay decoupled from the Slint
        // global (callers may be off the UI thread).
        let needle = self.inner.filter.lock().clone();
        self.inner.tracks_all.snapshot().ids_filtered(&needle)
    }

    /// Track ids of the Most Played grid, in card order. `play-track` and
    /// `shuffle-most-played` hand these to `player_play_tracks` so a card
    /// loads that grid rather than the Songs tab's list.
    ///
    /// Filtered through the same predicate `grids::apply::build_filtered_grids`
    /// builds the model with — the grid narrows with the hero search bar, so
    /// the raw cache would enqueue cards that aren't on screen.
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

    /// Whether a given artist appears in the cached Favorite Artists
    /// grid.
    pub fn fav_artist_known(&self, id: i64) -> bool {
        self.inner.fav_artists.lock().iter().any(|a| a.id == id)
    }

    /// Surgically flip `is_favorite` on a cached Songs-tab row so a
    /// single-row toggle reflects in the model without a full re-fetch.
    /// When `fav == false`, the row is removed entirely (parity with
    /// `library::favorites::get_favorite_tracks` which only returns
    /// `is_favorite = TRUE` rows).
    pub fn flip_or_remove_track(&self, id: i64, fav: bool) {
        if fav {
            self.inner.tracks_all.set_favorite(id, true);
        } else {
            self.inner.tracks_all.remove(id);
        }
    }

    /// Surgically set `rating` on a cached All Songs row. Unlike
    /// [`Self::flip_or_remove_track`], rating never affects membership (the
    /// list stays keyed on `is_favorite = TRUE`), so the row is only patched.
    pub fn flip_track_rating(&self, id: i64, rating: i32) {
        self.inner.tracks_all.set_rating(id, rating);
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<FavoritesUi>();
};

#[cfg(test)]
#[path = "tests/favorites_tests.rs"]
mod tests;
