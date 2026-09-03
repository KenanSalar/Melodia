//! The Albums page: a virtualized card grid, and the detail view a card drills
//! into.
//!
//! Rust owns the flat `Vec<AlbumStats>` behind `AlbumsUi::grid.data`, fetched
//! once, and rebuilds the grid model from it on every filter / sort /
//! column-count change with no DB hit. Chunking into `Albums.columns`-wide rows
//! is what makes the `ListView` virtualize, and each on-screen card pulls its
//! cover through `request-cover` rather than carrying a decoded image.
//!
//! The detail's cached `Vec<TrackListRow>` lets `play-row`, `select-row` and
//! `shuffle-album` recover ids and re-sort in memory without round-tripping the
//! Slint model — `BrowseUi::last_files`' shape.

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
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::ui::artwork_cache::BlurSpec;
use crate::ui::detail_artwork::{self, DetailArtwork};
use crate::ui::row_match::Needle;
use crate::ui::section_state::{SectionState, impl_detail_row_cache, impl_section_state_helpers};
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AlbumDetail, AlbumGridRow as UiAlbumGridRow, AlbumRow as UiAlbumRow, Albums, AppWindow,
    TrackListRow as UiTrackListRow,
};

use crate::ui::grid_prewarm::GRID_COVER_FALLBACK;
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

// `pub(super)` is `pub(in crate::ui)` here, which is exactly the reach these
// need: this slice's own `callbacks/`, plus the cross-slice `apply_detail_row_*`
// mirrors and the drill in `callbacks::cross_tab_nav`.
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
    let albums_ui =
        Arc::new(AlbumsUi::new(cx.cover_thumbs.clone(), detail_artwork::blur_spec(cx.app)));
    callbacks::wire(cx.app, cx.state, cx.view_state, &albums_ui);
    albums_ui
}

/// Rust-side state for the Albums grid and detail, shared between the UI
/// callbacks and the async fetchers. The two concerns are largely independent,
/// so each lives in its own sub-struct.
pub struct AlbumsUi {
    grid: AlbumGridState,
    detail: AlbumDetailState,
    /// The shared row tier, for the detail `TrackList`'s artwork column.
    cover_thumbs: Arc<CoverThumbs>,
    /// The grid card tier — private, with a resolution-derived cap. Separate
    /// from `cover_thumbs` so its larger buffers can't pollute the row LRU, and
    /// from `detail_artwork` because the tiles render at a different size.
    /// Released whenever the user leaves the section, re-warmed on return.
    grid_covers: Arc<CoverThumbs>,
    /// The detail header's `(cover, blur)` pair, one decode yielding both — see
    /// [`crate::ui::detail_artwork`]. Released beside `grid_covers`.
    detail_artwork: Arc<DetailArtwork>,
    /// Visibility + staleness + the mutation gate. See [`SectionState`].
    section: SectionState,
}

impl AlbumsUi {
    fn new(cover_thumbs: Arc<CoverThumbs>, hero_blur: Option<BlurSpec>) -> Self {
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
                GRID_COVER_FALLBACK,
                DEFAULT_GRID_COVER_CAP,
            )),
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
            section: SectionState::new(),
        }
    }

    /// Drop *everything* the section keeps resident — both cover tiers, the
    /// canonical grid data, the memoized indices, the cached detail rows and the
    /// selection shadow — then hand the freed pages back. Runs off the UI thread
    /// on section leave; the caller has already cleared the Slint model there,
    /// which a `VecModel` mutation can't be done from here. Re-entry re-fetches
    /// through [`fetch_grid`], and re-runs `open_album` if a detail was open.
    ///
    /// Race-correct against an in-flight [`fetch_grid`] two ways: the early
    /// `section_active()` short-circuits when a fast leave→re-enter beats this
    /// task to the gate, and the bulk writes happen under the gate with a
    /// *second* check inside it — so a parallel fetch either writes before we
    /// acquire, and we abandon, or waits and writes after. Never both.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.grid_covers.clear();
        self.detail_artwork.clear();
        {
            let _gate = self.section.gate();
            // The early check is a cheap pre-filter; this one is what makes the
            // wipe race-correct against `fetch_grid`'s gated write.
            if self.section_active() {
                return;
            }
            *self.grid.data.lock() = Arc::new(GridData::new(Vec::new()));
            *self.grid.index_cache.lock() = None;
            self.detail.tracks.lock().clear();
            self.detail.all_tracks.lock().clear();
            self.detail.applied_selection.lock().clear();
        }
        crate::services::platform::allocator::trim();
    }

    /// Drop just the grid tier, on opening an album: the grid unmounts the
    /// moment `AlbumDetail.album-id >= 0` flips, so its covers are neither
    /// visible nor queried. The header pair stays warm, and returning to the
    /// grid re-warms through [`Self::prewarm_visible_covers`].
    pub fn release_grid_covers(&self) {
        self.grid_covers.clear();
        crate::services::platform::allocator::trim();
    }

    /// The mirror image, on closing one: the detail's slots aren't queried again
    /// until the next `open_album`, and the grid the user is now looking at
    /// stays warm.
    pub fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        crate::services::platform::allocator::trim();
    }

    /// Re-decode the first screenful of grid covers, so a section enter over
    /// still-warm data paints cache hits rather than decoding inline on the UI
    /// thread. The post-wipe path goes through `fetch_grid`, which prewarms
    /// itself.
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

    /// Backs `Albums.request-cover`, so a card's cover is resolved only once it
    /// is on screen.
    pub fn grid_cover(&self, artwork_path: &str) -> slint::Image {
        self.grid_covers
            .get_or_schedule_opt(crate::ui::grid_prewarm::nonempty_artwork_path(artwork_path))
    }

    /// The inline sibling, for Artist Detail's Albums strip — its callback carries no generation,
    /// so a scheduled cover would have nothing to bring the card back.
    pub fn grid_cover_blocking(&self, artwork_path: &str) -> slint::Image {
        crate::ui::grid_prewarm::grid_cover_blocking(&self.grid_covers, artwork_path)
    }

    /// The grid tier itself, for a surface that borrows it — Artist Detail's
    /// Albums strip prewarms through this and resolves via [`Self::grid_cover`].
    /// One LRU, so the existing release sites clear it for both.
    pub fn grid_thumbs(&self) -> Arc<CoverThumbs> {
        self.grid_covers.clone()
    }
}

impl_section_state_helpers!(AlbumsUi);
impl_detail_row_cache!(AlbumsUi);

/// Hand the two globals their empty `VecModel`s. Later updates find them by
/// downcasting back.
fn install_models(ui: &AppWindow) {
    let grid: Rc<VecModel<UiAlbumGridRow>> = Rc::new(VecModel::default());
    ui.global::<Albums>().set_grid_rows(ModelRc::from(grid));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<AlbumDetail>().set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<AlbumDetail>().set_selected_ids(ModelRc::from(sel));
}

/// An `AlbumStats` as the `AlbumRow` an `EntityCard` and the detail header
/// render. Pure data — the cover comes through `request-cover` per visible card,
/// which is what makes this cheap enough to run for every album on every rebuild.
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

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<AlbumsUi>();
};

#[cfg(test)]
#[path = "tests/albums_tests.rs"]
mod tests;
