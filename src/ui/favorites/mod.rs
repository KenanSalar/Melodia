//! Favorites-view glue between Rust and Slint.
//!
//! Drives the `Favorites` Slint global (sidebar index 2) — a single
//! scrollable page composed of:
//!
//! * Hero — live 2×2 cover mosaic + count + total duration + the
//!   Shuffle / Columns pill. The mosaic refreshes whenever
//!   `library_changed_tx` ticks (favourite toggled, play count bumped,
//!   library scanned) so it always reflects the top-4 most-played
//!   favourites. The blur backdrop fades through the shared
//!   `Favorites.blur-img-{a,b}` dual-slot pattern.
//! * Two horizontal carousels — Most Played and Favorite Artists —
//!   composing `HorizontalCardStrip` with the per-strip data fetched
//!   from `library::favorites::*`. (No Favorite Albums carousel; the
//!   Albums tab covers that.)
//! * All Songs — a full `TrackList` bound to the post-filter
//!   `Favorites.tracks` model. Search is in-memory (the SQL fetch
//!   returns the entire sorted set once per `library_changed_tx` tick,
//!   then keystrokes re-walk the cached `tracks_all` without hitting
//!   `SQLite`).
//!
//! Cache discipline mirrors `src/ui/albums`: per-tier `CoverThumbs`
//! LRUs (the shared row tier plus dedicated tiers for the mosaic /
//! Most Played / Artists), released on Favorites-section leave — and,
//! for the two collapsible strips, on collapse — so the hidden view's
//! resident footprint drops to ~0. Re-enter
//! re-fetches via `library_changed_tx`-driven `mark_dirty` /
//! `take_dirty` round-trip.

mod hero;
mod sections;
mod selection;
mod state;
mod tracks;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::entities::artist::FavoriteArtist;
use crate::entities::track::{FavoriteStats, MostPlayedFavorite};
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AppWindow, EntityStripRow as UiEntityStripRow, Favorites, TrackListRow as UiTrackListRow,
};

use state::{
    ARTIST_THUMB_CAP, ARTIST_THUMB_SIZE, FavoritesUiState, MOSAIC_THUMB_CAP, MOSAIC_THUMB_SIZE,
    MOST_PLAYED_THUMB_CAP, MOST_PLAYED_THUMB_SIZE,
};

pub use hero::refresh_hero;
pub use sections::{apply_filtered_strips, refresh_strips};
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
    /// Shared row-tier (72 px) cache — used for the All Songs `TrackList`
    /// row column. Same instance every view consumes.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Mosaic-tile cache (128 px). Released on section leave; warm
    /// across mosaic refreshes inside one section visit.
    pub(super) mosaic_thumbs: Arc<CoverThumbs>,
    /// Most Played strip cache (180 px). Released on section leave.
    pub(super) most_played_thumbs: Arc<CoverThumbs>,
    /// Favorite Artists strip cache (200 px, circular cards). Released
    /// on section leave.
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
                MOST_PLAYED_THUMB_CAP,
            )),
            artist_thumbs: Arc::new(CoverThumbs::with_config(
                ARTIST_THUMB_SIZE,
                ARTIST_THUMB_CAP,
            )),
            section_active: AtomicBool::new(false),
            data_dirty: AtomicBool::new(false),
        }
    }

    pub fn set_section_active(&self, active: bool) {
        self.section_active.store(active, Ordering::Relaxed);
    }

    pub fn section_active(&self) -> bool {
        self.section_active.load(Ordering::Relaxed)
    }

    pub fn mark_dirty(&self) {
        self.data_dirty.store(true, Ordering::Release);
    }

    /// Atomically read-and-clear the dirty flag. The section-enter
    /// handler uses this to decide whether to re-fetch hero + strips +
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

    /// Drop just the Most Played strip cover cache. Called (off the UI
    /// thread) when that sub-section is collapsed: the strip's
    /// `HorizontalCardStrip` is unmounted from the layout tree by the
    /// `if !most-played-collapsed` gate, so its 180 px covers are no
    /// longer visible or queried via `request-most-played-cover`.
    /// Re-expanding re-decodes them lazily on card mount. Mirrors
    /// [`crate::ui::albums::AlbumsUi::release_grid_covers`].
    pub fn release_most_played_covers(&self) {
        self.most_played_thumbs.clear();
        crate::tasks::heap_trim::trim();
    }

    /// Drop just the Favorite Artists strip cover cache — the collapse
    /// counterpart for the `if !artists-collapsed` scroller. See
    /// [`Self::release_most_played_covers`].
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

    /// Lazy cover lookup for the Most Played strip cards. Routed via
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

    /// Track ids of the post-filter All Songs list, in display order.
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

    /// Track ids of the Most Played strip, in card order. `play-track`
    /// hands these to `player_play_tracks` so clicking a card loads the
    /// strip rather than the All Songs list below it.
    ///
    /// Filtered through the same predicate `apply_filtered_strips` builds
    /// the model with — the strip narrows with the hero search bar, so the
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
    /// strip.
    pub fn fav_artist_known(&self, id: i64) -> bool {
        self.inner.fav_artists.lock().iter().any(|a| a.id == id)
    }

    /// Surgically flip `is_favorite` on a cached All Songs row so a
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

/// Bind empty Slint `VecModel`s for the Most Played carousel, the
/// Favorite Artists scroller, the All Songs list, the selection set,
/// and the mosaic-path string list. Subsequent updates locate them by
/// downcasting back to `VecModel<T>` from the UI thread.
pub fn install_favorites_models(ui: &AppWindow) {
    let g = ui.global::<Favorites>();

    let most_played: Rc<VecModel<UiEntityStripRow>> = Rc::new(VecModel::default());
    g.set_most_played_rows(ModelRc::from(most_played));

    let artists: Rc<VecModel<UiEntityStripRow>> = Rc::new(VecModel::default());
    g.set_artist_rows(ModelRc::from(artists));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    g.set_tracks(ModelRc::from(tracks));

    let mosaic_paths: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
    g.set_mosaic_paths(ModelRc::from(mosaic_paths));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
}

/// Map a `MostPlayedFavorite` to its Slint strip row. Subtitle is the
/// artist name (mirrors the Tauri card). `play_count` rides in the
/// `play_count` slot so the strip's `show-play-count: true` reveals it.
pub fn to_slint_most_played_row(t: &MostPlayedFavorite) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(t.id),
        title: SharedString::from(t.title.as_str()),
        subtitle: SharedString::from(t.artist.as_deref().unwrap_or("")),
        artwork_path: SharedString::from(t.artwork_path.as_deref().unwrap_or("")),
        play_count: t.play_count,
    }
}

/// Map a `FavoriteArtist` + caller-supplied subtitle to its Slint strip
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
