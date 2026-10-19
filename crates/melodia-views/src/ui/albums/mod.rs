//! The Albums page: a virtualized card grid, and the detail view a card drills
//! into.
//!
//! The grid is a `PagedGrid` holding the albums' ids in the current sort and
//! filter and only the cards near the viewport, its loads `ui::card_grid`'s. Each
//! on-screen card pulls its cover through `request-cover` rather than carrying a
//! decoded image.
//!
//! The detail lists its album as a keyset in a [`TrackRows`], whose
//! [`DetailCache`] holds the sort and filter its loads run under.

mod callbacks;
mod detail;
mod grid;

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Global as _, ModelRc, SharedString, VecModel};

use crate::ui::artwork_cache::BlurSpec;
use crate::ui::card_grid::{self, GridLoads};
use crate::ui::detail_artwork::{self, DetailArtwork};
use crate::ui::paged_rows::PagedRow;
use crate::ui::paged_rows::grid::GridRow;
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::track_detail::DetailCache;
use crate::ui::util::{clamp_i64_to_i32, opt_shared};
use crate::ui::view_ctx::ViewCtx;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::album::AlbumStats;
use melodia_ui::{
    AlbumDetail, AlbumGridRow as UiAlbumGridRow, AlbumRow as UiAlbumRow, Albums, AppWindow,
};

use grid::AlbumsGrid;

// Reached from outside the slice: `ui::nav_history` replays a walk into a
// detail, `boot::ui_setup` seeds the persisted one, and its
// `initial_grid_fetch!` kicks the first load after the window is shown
// — which is why `reload_grid` can't fold into `install` with the rest.
pub use detail::{open_album_with, seed_detail_from_settings};
pub use grid::reload_grid;

// `pub(super)` is `pub(in crate::ui)` here, which is exactly the reach these
// need: this slice's own `callbacks/` and the drill in `callbacks::cross_tab_nav`.
pub(super) use detail::{open_album, refresh_detail, restore_detail};

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
    let (grid, detail) = install_models(cx.app, cx.state);
    let albums_ui = Arc::new(AlbumsUi::new(
        grid,
        detail,
        cx.cover_thumbs.clone(),
        detail_artwork::blur_spec(cx.app),
    ));
    callbacks::wire(cx.app, cx.state, cx.view_state, &albums_ui);
    albums_ui
}

/// Rust-side state for the Albums grid and detail, shared between the UI
/// callbacks and the async loads. The two concerns are largely independent.
pub struct AlbumsUi {
    grid: GridLoads,
    detail: DetailCache<AlbumDetail<'static>>,
    /// The shared row tier, for the detail `TrackList`'s artwork column. The grid's cards draw
    /// from `ui::grid_prewarm::tier()`, which no view holds.
    cover_thumbs: Arc<CoverThumbs>,
    /// The detail header's `(cover, blur)` pair, one decode yielding both — see
    /// [`crate::ui::detail_artwork`]. Handed back beside the grid's covers.
    detail_artwork: Arc<DetailArtwork>,
    /// Visibility + staleness + the mutation gate. See [`SectionState`].
    section: SectionState,
}

impl AlbumsUi {
    fn new(
        grid: GridLoads,
        detail: DetailCache<AlbumDetail<'static>>,
        cover_thumbs: Arc<CoverThumbs>,
        hero_blur: Option<BlurSpec>,
    ) -> Self {
        Self {
            grid,
            detail,
            cover_thumbs,
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
            section: SectionState::new(),
        }
    }

    /// Hand back what the section keeps resident off the UI thread, the detail's artwork and the
    /// grid tier's pixels down to proxies, then the freed pages. Runs on section leave, the caller
    /// having released the grid and the detail's rows on the UI thread already. Re-entry reloads
    /// the grid, and re-runs `open_album` if a detail was open.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        crate::ui::grid_prewarm::shrink_covers();
        self.detail_artwork.clear();
        melodia_platform::services::platform::allocator::trim();
    }

    /// The mirror image, on closing one: the detail's slots aren't queried again
    /// until the next `open_album`, and the grid the user is now looking at
    /// stays warm.
    pub fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        melodia_platform::services::platform::allocator::trim();
    }

    /// Decodes the covers the grid's last load opened on, for a grid coming back from under a
    /// detail. Does nothing unless the grid is what is drawn ([`SectionState::grid_on_screen`]).
    /// Blocking.
    pub fn prewarm_visible_covers(&self) {
        card_grid::prewarm_seeded::<AlbumsGrid>(self);
    }

    /// Album id currently open in the detail view (`-1` = grid).
    pub fn detail_album_id(&self) -> i64 {
        self.detail.id()
    }
}

impl_section_state_helpers!(AlbumsUi);

/// Hand the two globals their empty models, each installed once: the grid a `PagedGrid`, the
/// detail's tracks a [`TrackRows`] for every album it will show. Later updates find them by
/// downcasting back.
fn install_models(
    ui: &AppWindow,
    state: &AppState,
) -> (GridLoads, DetailCache<AlbumDetail<'static>>) {
    let grid = card_grid::install::<AlbumsGrid>(state, ui);
    let loads = GridLoads::new(grid.scroll_point());
    ui.global::<Albums>().set_grid_rows(ModelRc::from(grid));

    let g = ui.global::<AlbumDetail>();
    let tracks: Rc<TrackRows> = TrackRows::new(track_rows::fetcher(state));
    let detail = DetailCache::new(g.as_weak(), tracks.scroll_point());
    g.set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
    (loads, detail)
}

impl GridRow for UiAlbumGridRow {
    type Card = UiAlbumRow;

    fn of(albums: ModelRc<UiAlbumRow>) -> Self {
        Self { albums }
    }
}

impl PagedRow for UiAlbumRow {
    // Blank but live: a card clicked before its fetch lands still opens, plays and picks by id.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }
}

/// An `AlbumStats` as the `AlbumRow` an `EntityCard` and the detail header
/// render. Pure data — the cover comes through `request-cover` per visible card.
pub fn to_slint_album_row(a: &AlbumStats) -> UiAlbumRow {
    UiAlbumRow {
        id: clamp_i64_to_i32(a.id),
        name: SharedString::from(a.name.as_str()),
        artist_id: clamp_i64_to_i32(a.artist_id),
        artist_name: SharedString::from(a.artist_name.as_str()),
        year: a.year.unwrap_or(0),
        track_count: a.track_count,
        total_duration_ms: i32::try_from(a.total_duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
        artwork_path: opt_shared(a.artwork_path.as_deref()),
    }
}

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<AlbumsUi>();
};
