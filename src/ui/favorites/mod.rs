//! Favorites-view glue between Rust and Slint.
//!
//! Drives the `Favorites` Slint global (sidebar index 2) — a pinned hero
//! over three mutually exclusive sub-views:
//!
//! * Hero — live 2×2 cover mosaic + count + total duration + the per-tab
//!   action pill, under a header row carrying the tab bar and the filter.
//!   The mosaic refreshes whenever `library_changed_tx` ticks (favourite
//!   toggled, play count bumped, library scanned) so it always reflects the
//!   top-4 most-played favourites. The blur backdrop fades through the
//!   shared `Favorites.blur-img-{a,b}` dual-slot pattern.
//! * Songs — a full `TrackList` bound to the post-filter
//!   `Favorites.tracks` model. Search is in-memory (the SQL fetch
//!   returns the entire sorted set once per `library_changed_tx` tick,
//!   then keystrokes re-walk the cached `tracks_all` without hitting
//!   `SQLite`).
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

mod hero;
mod sections;
mod selection;
mod state;
mod tracks;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::entities::artist::FavoriteArtist;
use crate::entities::track::{FavoriteStats, MostPlayedFavorite};
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AppWindow, EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow, Favorites,
    TrackListRow as UiTrackListRow,
};

use state::{
    ARTIST_THUMB_SIZE, FavoritesUiState, GRID_PREWARM_AHEAD, GRID_THUMB_CAP, MOSAIC_THUMB_CAP,
    MOSAIC_THUMB_SIZE, MOST_PLAYED_THUMB_SIZE,
};

/// Which Favorites sub-view is mounted.
///
/// The indices themselves are declared once, in `curated.slint`'s `tab-*`
/// constants; [`tab_from_index`] resolves one to this on the UI thread, so no
/// Rust file restates them. Off-thread callers (the fetchers) read the shadow
/// rather than the global, which they can't touch anyway.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FavoritesTab {
    Songs,
    MostPlayed,
    Artists,
}

impl FavoritesTab {
    /// Storage code for the atomic shadow. Deliberately *not* the Slint index
    /// — that lives in the Slint, and these two numbering schemes agreeing
    /// today is a coincidence worth not depending on.
    fn as_code(self) -> u8 {
        match self {
            Self::Songs => 0,
            Self::MostPlayed => 1,
            Self::Artists => 2,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Self::MostPlayed,
            2 => Self::Artists,
            _ => Self::Songs,
        }
    }
}

/// Resolve a `Favorites.tab-idx` value against the global's own `tab-*`
/// constants. UI thread only — that's where the global is reachable.
pub fn tab_from_index(g: &Favorites<'_>, idx: i32) -> FavoritesTab {
    if idx == g.get_tab_most_played() {
        FavoritesTab::MostPlayed
    } else if idx == g.get_tab_artists() {
        FavoritesTab::Artists
    } else {
        FavoritesTab::Songs
    }
}

pub use hero::refresh_hero;
pub use sections::{apply_filtered_grids, refresh_grids};
pub use selection::{clear_selection, handle_select_row};
pub use tracks::{
    apply_filtered_tracks, apply_row_rating, current_filter, current_sort, refresh_tracks,
    set_filter, set_sort,
};

/// Rust-side state for the Favorites view. Shared between the UI
/// callbacks (`wire_favorites`) and the async fetchers behind an
/// `Arc<FavoritesUi>` — `Send + Sync`.
pub struct FavoritesUi {
    inner: FavoritesUiState,
    /// Shared row-tier (72 px) cache — used for the Songs tab's
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
    /// Section-visible shadow — mirrors
    /// `Nav.selected-index == NAV_FAVORITES && !Nav.now-playing-open`.
    /// Gates the library-changed subscriber's refresh so a background
    /// tick doesn't repaint a hidden view.
    section_active: AtomicBool,
    /// Sticky "data is stale, refetch on next section enter". Set on
    /// every `library_changed_tx` tick that fires while the view is
    /// hidden, plus on section leave; cleared by `take_dirty`.
    data_dirty: AtomicBool,
    /// Synchronous shadow of `Favorites.tab-idx`, as a [`FavoritesTab`].
    /// The off-thread fetchers decide which cover tier to warm from this —
    /// only one grid is ever mounted, so warming both is half the decodes
    /// and twice the resident buffers for nothing.
    active_tab: AtomicU8,
}

impl FavoritesUi {
    pub fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            inner: FavoritesUiState::new(),
            cover_thumbs,
            mosaic_thumbs: Arc::new(CoverThumbs::with_config(
                MOSAIC_THUMB_SIZE,
                MOSAIC_THUMB_CAP,
            )),
            most_played_thumbs: Arc::new(CoverThumbs::with_config(
                MOST_PLAYED_THUMB_SIZE,
                GRID_THUMB_CAP,
            )),
            artist_thumbs: Arc::new(CoverThumbs::with_config(
                ARTIST_THUMB_SIZE,
                GRID_THUMB_CAP,
            )),
            section_active: AtomicBool::new(false),
            data_dirty: AtomicBool::new(false),
            active_tab: AtomicU8::new(FavoritesTab::Songs.as_code()),
        }
    }

    pub fn set_section_active(&self, active: bool) {
        self.section_active.store(active, Ordering::Relaxed);
    }

    pub fn section_active(&self) -> bool {
        self.section_active.load(Ordering::Relaxed)
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

    /// First-screenful cover paths for a grid tab, in display order.
    ///
    /// Deduped and capped by the shared [`crate::ui::grid_prewarm`] helper —
    /// the cap bounds *kept paths*, not input items, so an uncapped grid over
    /// a large library doesn't allocate a `PathBuf` per unique cover just to
    /// keep the first two rows.
    pub fn first_screenful_paths(&self, tab: FavoritesTab) -> Vec<PathBuf> {
        match tab {
            FavoritesTab::MostPlayed => crate::ui::grid_prewarm::unique_artwork_paths(
                self.inner.most_played.lock().iter().map(|t| t.artwork_path.as_deref()),
                GRID_PREWARM_AHEAD,
            ),
            FavoritesTab::Artists => crate::ui::grid_prewarm::unique_artwork_paths(
                self.inner.fav_artists.lock().iter().map(|a| a.image_path.as_deref()),
                GRID_PREWARM_AHEAD,
            ),
            FavoritesTab::Songs => Vec::new(),
        }
    }

    /// Decode a grid tab's first screenful into its tier. Blocking — call it
    /// from `spawn_blocking`, never on the UI thread.
    ///
    /// The Songs tab is a no-op: its row covers come from the shared 72 px
    /// row tier, which `refresh_tracks` already warms.
    pub fn prewarm_tab_covers(&self, tab: FavoritesTab) {
        let paths = self.first_screenful_paths(tab);
        if paths.is_empty() {
            return;
        }
        match tab {
            FavoritesTab::MostPlayed => self.most_played_thumbs.prewarm(&paths),
            FavoritesTab::Artists => self.artist_thumbs.prewarm(&paths),
            FavoritesTab::Songs => {}
        }
    }

    pub fn mark_dirty(&self) {
        self.data_dirty.store(true, Ordering::Release);
    }

    /// Atomically read-and-clear the dirty flag. The section-enter
    /// handler uses this to decide whether to re-fetch hero + grids +
    /// tracks; the live-refresh subscriber sets it whenever a
    /// `library_changed_tx` tick arrives while the view is hidden.
    pub fn take_dirty(&self) -> bool {
        self.data_dirty.swap(false, Ordering::AcqRel)
    }

    /// Forget the last-composed mosaic covers, so the next refresh recomposes
    /// the hero blur instead of skipping on an unchanged cover set.
    ///
    /// The section-leave caller sits beside the `blur-img-*` wipe rather than in
    /// [`Self::release_section_state`]: that one bails out when the user has
    /// already come back, but the wipe is unconditional — leaving the guard set
    /// against a hero that no longer has a blur to guard.
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.lock().clear();
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
    /// per-section LRUs first, then mutated state, then a
    /// `heap_trim::trim` so glibc hands the freed pages back.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.mosaic_thumbs.clear();
        self.most_played_thumbs.clear();
        self.artist_thumbs.clear();
        self.inner.tracks_all.lock().clear();
        *self.inner.stats.lock() = FavoriteStats {
            count: 0,
            total_duration_ms: 0,
            artwork_paths: Vec::new(),
        };
        self.inner.most_played.lock().clear();
        self.inner.fav_artists.lock().clear();
        self.inner.applied_selection.lock().clear();
        crate::tasks::heap_trim::trim();
    }

    /// Drop just the Most Played grid's cover cache. Called (off the UI
    /// thread) when another tab is picked: the `if tab-idx == …` gate has
    /// unmounted the grid, so its covers are no longer visible or queried
    /// via `request-most-played-cover`. Coming back re-decodes them lazily
    /// on card mount. Mirrors
    /// [`crate::ui::albums::AlbumsUi::release_grid_covers`], which does the
    /// same on drill-in.
    pub fn release_most_played_covers(&self) {
        self.most_played_thumbs.clear();
        crate::tasks::heap_trim::trim();
    }

    /// Drop just the Favorite Artists grid's cover cache — the tab-leave
    /// counterpart of [`Self::release_most_played_covers`], and also called
    /// when a card drills into Artist Detail.
    pub fn release_artist_covers(&self) {
        self.artist_thumbs.clear();
        crate::tasks::heap_trim::trim();
    }

    pub(crate) fn state(&self) -> &FavoritesUiState {
        &self.inner
    }

    /// Lazy cover lookup for the hero 2x2 mosaic tiles. Routed via
    /// `Favorites.request-mosaic-cover`.
    pub fn mosaic_cover(&self, artwork_path: &str) -> Image {
        self.mosaic_thumbs
            .get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Lazy cover lookup for the Most Played grid cards. Routed via
    /// `Favorites.request-most-played-cover`.
    pub fn most_played_cover(&self, artwork_path: &str) -> Image {
        self.most_played_thumbs
            .get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Lazy cover lookup for the Favorite Artists circular cards.
    pub fn artist_cover(&self, artwork_path: &str) -> Image {
        self.artist_thumbs
            .get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Track ids of the post-filter Songs tab, in display order.
    /// `shuffle-all` / `play-row` use this to recover ids without
    /// round-tripping the Slint model.
    pub fn filtered_track_ids(&self) -> Vec<i64> {
        // The post-filter list is the one currently bound to the Slint
        // model; the unfiltered cache is in `tracks_all`. We re-walk
        // the cache + filter here to stay decoupled from the Slint
        // global (callers may be off the UI thread).
        let needle = self.inner.filter.lock().to_lowercase();
        let all = self.inner.tracks_all.lock();
        all.iter()
            .filter(|r| crate::ui::detail_filter::track_matches(r, &needle))
            .map(|r| r.id)
            .collect()
    }

    /// Track ids of the Most Played grid, in card order. `play-track` and
    /// `shuffle-most-played` hand these to `player_play_tracks` so a card
    /// loads that grid rather than the Songs tab's list.
    ///
    /// Filtered through the same predicate `apply_filtered_grids` builds
    /// the model with — the grid narrows with the hero search bar, so the
    /// raw cache would enqueue cards that aren't on screen.
    pub fn most_played_track_ids(&self) -> Vec<i64> {
        let needle = self.inner.filter.lock().to_lowercase();
        self.inner
            .most_played
            .lock()
            .iter()
            .filter(|t| crate::ui::detail_filter::most_played_matches(t, &needle))
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
        let mut tracks = self.inner.tracks_all.lock();
        if !fav {
            tracks.retain(|r| r.id != id);
        } else if let Some(r) = tracks.iter_mut().find(|r| r.id == id) {
            r.is_favorite = true;
        }
    }

    /// Surgically set `rating` on a cached All Songs row. Unlike
    /// [`Self::flip_or_remove_track`], rating never affects membership (the
    /// list stays keyed on `is_favorite = TRUE`), so the row is only patched.
    pub fn flip_track_rating(&self, id: i64, rating: i32) {
        if let Some(r) = self.inner.tracks_all.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
    }
}

/// Seed the active tab from `views.json`, clamped against the Slint-declared
/// `tab-count` (see [`crate::ui::tab_bar::clamp_tab`]).
///
/// Seeds **both** the Slint property and the [`FavoritesUi`] shadow, which is
/// why it takes the handle and why it is called from `install_views` rather
/// than alongside its siblings in `hydrate_ui_from_settings` — that runs after
/// the handle has gone out of scope, and a shadow left at its `Songs` default
/// would have the first fetch warm the wrong tier for a session that resumes
/// on a grid tab.
pub fn seed_tab(ui: &AppWindow, fav_ui: &FavoritesUi, persisted_tab: i32) {
    let g = ui.global::<Favorites>();
    let clamped = crate::ui::tab_bar::clamp_tab(persisted_tab, g.get_tab_count());
    g.set_tab_idx(clamped);
    fav_ui.set_active_tab(tab_from_index(&g, clamped));
}

/// Retune both grid-tier cover caches to the real display resolution. Called
/// once at startup after the winit window is live, alongside the entity
/// grids' own tuning — the tabs draw the same card at the same size, so they
/// take the same band.
///
/// Both, even though only one is ever warm: which tab the user resumes on
/// isn't known until `seed_tab`, and resizing an empty LRU costs nothing.
pub fn tune_cache_for_display(app: &AppWindow, fav_ui: &FavoritesUi) {
    let cap = crate::ui::grid_prewarm::cover_cap_for_window(app, GRID_THUMB_CAP);
    fav_ui.most_played_thumbs.resize(cap);
    fav_ui.artist_thumbs.resize(cap);
    log::debug!("ui::favorites grid-cover cache caps tuned to {cap}");
}

/// Bind empty Slint `VecModel`s for the two grid tabs, the Songs list, the
/// selection set, and the mosaic-path string list. Subsequent updates locate
/// them by downcasting back to `VecModel<T>` from the UI thread.
pub fn install_favorites_models(ui: &AppWindow) {
    let g = ui.global::<Favorites>();

    let most_played: Rc<VecModel<UiEntityGridRow>> = Rc::new(VecModel::default());
    g.set_most_played_rows(ModelRc::from(most_played));

    let artists: Rc<VecModel<UiEntityGridRow>> = Rc::new(VecModel::default());
    g.set_artist_rows(ModelRc::from(artists));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    g.set_tracks(ModelRc::from(tracks));

    let mosaic_paths: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
    g.set_mosaic_paths(ModelRc::from(mosaic_paths));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
}

/// Map a `MostPlayedFavorite` to its Slint card row. Subtitle is the
/// artist name. `play_count` rides in the `play_count` slot so the grid's
/// `show-play-count: true` reveals the badge.
pub fn to_slint_most_played_row(t: &MostPlayedFavorite) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(t.id),
        title: SharedString::from(t.title.as_str()),
        subtitle: SharedString::from(t.artist.as_deref().unwrap_or("")),
        artwork_path: SharedString::from(t.artwork_path.as_deref().unwrap_or("")),
        play_count: t.play_count,
    }
}

/// Map a `FavoriteArtist` + caller-supplied subtitle to its Slint card
/// row. The subtitle is the translated "{n} favorite[s]" count line and
/// must be resolved on the UI thread via `Favorites.artist-favorite-subtitle(count)`
/// (Slint 1.16 doesn't expose `translate_from_bundle` to Rust). `play_count`
/// is unused.
pub fn to_slint_fav_artist_row(a: &FavoriteArtist, subtitle: SharedString) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle,
        artwork_path: SharedString::from(a.image_path.as_deref().unwrap_or("")),
        play_count: 0,
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
