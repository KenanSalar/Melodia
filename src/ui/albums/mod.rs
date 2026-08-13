//! Albums-view glue between Rust and Slint.
//!
//! Two Slint globals are driven from here:
//!
//! * `Albums` — the responsive album-card grid. Rust owns a flat,
//!   name-sorted `Vec<AlbumStats>` (plus pre-lowercased sort keys)
//!   behind `AlbumsUi::grid.data` (fetched once from `album_stats`); the
//!   grid model is rebuilt from it on every filter / sort / column-count
//!   change *without* a DB hit. The grid is virtualized by row: Rust chunks
//!   the filtered+sorted list into `Albums.columns`-wide rows of
//!   `AlbumGridRow`, so the Slint `ListView` only instantiates on-screen
//!   rows — and each on-screen card pulls its cover lazily via the
//!   `request-cover` callback rather than carrying a decoded image.
//!
//! * `AlbumDetail` — the full album detail view. `open_album` fetches the
//!   header + track list; the cached `Vec<TrackListRow>` in
//!   `AlbumsUi::detail.tracks` lets `play-row` / `select-row` /
//!   `shuffle-album` recover ids and re-sort in memory without
//!   round-tripping the Slint model (mirrors `BrowseUi::last_files`).
//!
//! Cross-thread layout mirrors `tracks.rs` / `browse.rs`: `AlbumsUi` is
//! `Send + Sync`, cloned into callbacks / tokio tasks; Slint properties and
//! models are only touched from the UI thread, reached via
//! `Weak<AppWindow>::upgrade_in_event_loop`.

mod callbacks;
mod detail;
mod grid;
mod selection;
mod state;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::album::AlbumStats;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::detail_artwork::DetailArtwork;
use crate::ui::row_match::Needle;
use crate::ui::section_state::SectionState;
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AlbumDetail, AlbumGridRow as UiAlbumGridRow, AlbumRow as UiAlbumRow, Albums, AppWindow,
    TrackListRow as UiTrackListRow,
};

use crate::ui::grid_prewarm::GRID_COVER_SIZE;
use state::{AlbumDetailState, AlbumGridState, DEFAULT_GRID_COVER_CAP, GridData};

#[cfg(test)]
use grid::compute_indices;
#[cfg(test)]
use state::GridIndexCache;

// Reached from outside the slice: `ui::nav_history` replays a walk into a
// detail, `boot::ui_setup` seeds the persisted one and retunes the cover cap,
// and its `initial_grid_fetch!` kicks the first fetch after the window is shown
// — which is why `fetch_grid` can't fold into `install` with the rest.
pub use detail::{open_album_with, seed_detail_from_settings};
pub use grid::{fetch_grid, tune_cache_for_display};

// Reached only from this slice's own `callbacks/`, which used to live two
// modules away, plus the cross-slice `apply_detail_row_*` mirrors in
// `callbacks::now_playing` and the drill in `callbacks::cross_tab_nav`.
// `pub(super)` is `pub(in crate::ui)` here, which is exactly that reach.
pub(super) use detail::{
    apply_detail_row_favorite, apply_detail_row_rating, apply_filtered_detail, clear_detail,
    open_album, refresh_detail, resort_detail, set_filter,
};
pub(super) use grid::rebuild_grid;
pub(super) use selection::{clear_selection, handle_select_row};

/// Install the Albums grid + detail models, build the handle, and wire every
/// `Albums.*` / `AlbumDetail.*` callback to it.
///
/// The returned handle is read by `install_views` for the four things that
/// genuinely outlive this call — the persisted-detail seed, the cover retune,
/// the `ui_handles` registry, and the two peers that need a live `AlbumsUi`
/// ([`crate::ui::artists`] and [`crate::ui::search`]). It is not a keepalive:
/// every wired closure clones its own strong `Arc`, and those closures are
/// owned by the `AppWindow` for the life of the app.
pub fn install(cx: ViewCtx<'_>) -> Arc<AlbumsUi> {
    install_models(cx.app);
    let albums_ui = Arc::new(AlbumsUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, &albums_ui);
    albums_ui
}

/// Rust-side state for the Albums grid + detail views. Shared between the
/// UI callbacks (`callbacks::wire`) and the async fetchers. The grid and detail
/// concerns are largely independent — each lives in its own sub-struct.
pub struct AlbumsUi {
    grid: AlbumGridState,
    detail: AlbumDetailState,
    /// Row-tier cache shared with Tracks / Browse / now-playing-bar
    /// — backs the small artwork column of the detail view's `TrackList`.
    cover_thumbs: Arc<CoverThumbs>,
    /// Grid-tier (`GRID_COVER_SIZE`) cache for the Albums grid card tiles —
    /// private to this view, resolution-derived cap. Kept separate from
    /// `cover_thumbs` for the same reason `NowPlayingArtwork` is (mixing
    /// large buffers into the row LRU would pollute it), and separate from
    /// `detail_artwork` because the grid tiles render larger. Released
    /// entirely (see [`Self::release_section_state`]) whenever the user
    /// leaves the Albums section, and re-warmed on return.
    grid_covers: Arc<CoverThumbs>,
    /// Detail-tier `(cover, blur)` pair cache for the Album Detail header.
    /// One decode per album yields both the sharp header tile and the
    /// heavily-blurred hero backdrop — see [`crate::ui::detail_artwork`].
    /// Released on section exit alongside `grid_covers`.
    detail_artwork: Arc<DetailArtwork>,
    /// Section-visibility + staleness bookkeeping (on-screen shadow, dirty
    /// flag, and the wipe/fetch mutation gate). See [`SectionState`].
    section: SectionState,
}

impl AlbumsUi {
    fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            grid: AlbumGridState {
                data: Mutex::new(Arc::new(GridData::new(Vec::new()))),
                index_cache: Mutex::new(None),
            },
            detail: AlbumDetailState {
                tracks: Mutex::new(Vec::new()),
                all_tracks: Mutex::new(Vec::new()),
                album_id: Mutex::new(-1),
                applied_selection: Mutex::new(HashSet::new()),
                filter: Mutex::new(Needle::default()),
            },
            cover_thumbs,
            grid_covers: Arc::new(CoverThumbs::with_config(
                GRID_COVER_SIZE,
                DEFAULT_GRID_COVER_CAP,
            )),
            detail_artwork: Arc::new(DetailArtwork::new()),
            section: SectionState::new(),
        }
    }

    /// Mirror the Albums-section-visible flag (`section-active-changed`).
    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    /// Whether the Albums section is currently on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Drop *everything* the Albums section is keeping resident — both
    /// cover LRUs, the canonical grid data (`Vec<AlbumStats>` +
    /// pre-lowercased keys), the memoized filter/sort indices, the cached
    /// detail track rows, and the applied-selection shadow — then hand the
    /// freed pages back to the OS. Called (off the UI thread) when the user
    /// leaves the Albums section so the hidden view's resident footprint
    /// drops to ~0. Re-entry re-fetches via [`fetch_grid`]; if a detail was
    /// open (`AlbumDetail.album-id >= 0`), the caller re-runs `open_album`
    /// to repopulate it. The Slint `Albums.grid-rows` model itself is
    /// cleared synchronously by the caller on the UI thread before this
    /// runs (a `VecModel<AlbumGridRow>` mutation can't happen here).
    ///
    /// Race-correct against an in-flight [`fetch_grid`]:
    ///
    /// * An early `section_active()` check short-circuits when a fast
    ///   leave→re-enter beats this task to the gate (no LRU clear, no wipe).
    /// * The bulk-state writes happen under `mutation_gate`, with a
    ///   re-check of `section_active()` after acquiring the gate, so a
    ///   parallel `fetch_grid` either (a) writes before we acquire and we
    ///   abandon, or (b) waits for our gate and writes after — never both
    ///   "writes fresh, then we wipe it" *and* lets the UI repaint observe
    ///   the wiped state.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.grid_covers.clear();
        self.detail_artwork.clear();
        {
            let _gate = self.section.gate();
            // The early `section_active()` is a cheap pre-filter; this
            // re-check inside the gate is what makes the wipe race-correct
            // against `fetch_grid`'s gated write.
            if self.section_active() {
                return;
            }
            *self.grid.data.lock() = Arc::new(GridData::new(Vec::new()));
            *self.grid.index_cache.lock() = None;
            self.detail.tracks.lock().clear();
            self.detail.all_tracks.lock().clear();
            self.detail.applied_selection.lock().clear();
        }
        crate::tasks::heap_trim::trim();
    }

    /// Mark the cached grid + detail data as "must be re-fetched on the
    /// next section-enter". Called synchronously on section-leave (UI
    /// thread, *before* spawning the release task) so the flag is observed
    /// by the section-enter handler even if release is still in flight.
    /// See [`Self::take_dirty`].
    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag. Returns `true` iff a leave
    /// has marked the data stale since the last `take_dirty`. The
    /// section-enter handler uses this to decide between a full
    /// `fetch_grid` (true) and a cheap `prewarm_visible_covers` (false —
    /// initial enter after the boot pre-fetch). Race-free against an
    /// in-flight wipe: `mark_dirty` lands on the UI thread before the
    /// release task is even spawned, so any subsequent enter observes it.
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
    }

    /// Drop just the grid-tier cover cache. Called (off the UI thread)
    /// when the user opens an album: the grid view is unmounted from
    /// the layout tree the moment `AlbumDetail.album-id >= 0` flips
    /// (see `app-window.slint`'s `if` gate), so its covers are not
    /// visible and not queried via `request-cover`. The detail header /
    /// blur pair stays warm. The next return to the grid re-warms via
    /// [`Self::prewarm_visible_covers`].
    pub fn release_grid_covers(&self) {
        self.grid_covers.clear();
        crate::tasks::heap_trim::trim();
    }

    /// Drop just the detail-tier `(cover, blur)` pair cache. Called (off
    /// the UI thread) when the user closes a detail view: the detail
    /// view is unmounted and its cover/blur slots aren't queried again
    /// until the next `open_album`. The grid cache stays warm (the user
    /// is now looking at it).
    pub fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        crate::tasks::heap_trim::trim();
    }

    /// Re-decode the first screenful of grid covers into the grid-tier
    /// cache. Called (off the UI thread) on the section-enter path when
    /// the grid data is still warm (initial enter after the boot
    /// pre-fetch), so the visible cards are cache hits instead of decoding
    /// inline on the UI thread. The post-wipe path goes through
    /// `fetch_grid` instead, which prewarms internally. No-op if the grid
    /// data hasn't been fetched yet.
    pub fn prewarm_visible_covers(&self) {
        let data = self.grid.data.lock().clone();
        let unique = grid::first_screenful_paths(&data);
        if !unique.is_empty() {
            self.grid_covers.prewarm(&unique);
        }
    }

    /// Album id currently open in the detail view (`-1` = grid).
    pub fn detail_album_id(&self) -> i64 {
        *self.detail.album_id.lock()
    }

    /// Track ids of the **displayed** detail list, in display order — the
    /// filter-applied subset when a search is active, otherwise the full
    /// album. `play-row` / shuffle / add-to-queue pass these straight to
    /// `player_play_tracks`, so those actions operate on the visible rows.
    pub fn detail_track_ids(&self) -> Vec<i64> {
        self.detail.tracks.lock().iter().map(|r| r.id).collect()
    }

    /// Surgically flip `is_favorite` on the cached detail row so a
    /// single-row favourite toggle doesn't need a full re-fetch. Both the
    /// displayed `tracks` cache and the canonical `all_tracks` set are
    /// touched so a later `apply_filtered_detail` rebuild keeps the star.
    pub fn flip_detail_favorite(&self, id: i64, fav: bool) {
        if let Some(r) = self.detail.tracks.lock().iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
        if let Some(r) = self.detail.all_tracks.lock().iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
    }

    /// Star-rating analogue of [`Self::flip_detail_favorite`] — set `rating`
    /// on both the displayed `tracks` cache and the canonical `all_tracks` set.
    pub fn flip_detail_rating(&self, id: i64, rating: i32) {
        if let Some(r) = self.detail.tracks.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
        if let Some(r) = self.detail.all_tracks.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
    }

    /// Lazy cover lookup for an Albums **grid card** — backs
    /// `Albums.request-cover`, so a cover is decoded (or LRU-hit) only when
    /// the card is on screen. Resolves against the grid-tier
    /// (`GRID_COVER_SIZE`) cache.
    pub fn grid_cover(&self, artwork_path: &str) -> slint::Image {
        self.grid_covers.get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Shared handle to the grid-tier cover cache, for surfaces that
    /// borrow it (Artist Detail's Albums strip resolves its cards via
    /// [`Self::grid_cover`] and prewarms through this handle). Same LRU —
    /// no second cache, and the existing release sites clear it for both
    /// surfaces.
    pub fn grid_thumbs(&self) -> Arc<CoverThumbs> {
        self.grid_covers.clone()
    }
}

/// Build the empty `VecModel`s the Albums grid, the detail track list, and
/// the detail selection need, and hand them to the Slint globals as
/// `ModelRc`s. Subsequent updates locate them by downcasting back to
/// `VecModel<T>`.
fn install_models(ui: &AppWindow) {
    let grid: Rc<VecModel<UiAlbumGridRow>> = Rc::new(VecModel::default());
    ui.global::<Albums>().set_grid_rows(ModelRc::from(grid));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<AlbumDetail>().set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<AlbumDetail>().set_selected_ids(ModelRc::from(sel));
}

/// Convert an `AlbumStats` into the Slint `AlbumRow` an `EntityCard` (and
/// the detail header) renders. Pure data — the cover is *not* decoded here:
/// it's pulled lazily per visible card via the `request-cover` callback, so
/// this is cheap enough to run for every album on every grid rebuild.
pub fn to_slint_album_row(a: &AlbumStats) -> UiAlbumRow {
    UiAlbumRow {
        id: clamp_i64_to_i32(a.id),
        name: SharedString::from(a.name.as_str()),
        artist_name: SharedString::from(a.artist_name.as_str()),
        year: a.year.unwrap_or(0),
        track_count: a.track_count,
        total_duration_ms: i32::try_from(a.total_duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
        artwork_path: SharedString::from(a.artwork_path.as_deref().unwrap_or("")),
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<AlbumsUi>();
};

#[cfg(test)]
#[path = "tests/albums_tests.rs"]
mod tests;
