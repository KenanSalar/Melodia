//! The Albums page: a `PagedGrid` of cards loaded through `ui::card_grid`, each pulling its cover
//! through `request-cover`, and the detail a card drills into, which lists its album as a
//! [`TrackRows`] under the sort and filter its [`DetailCache`] holds.

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

pub use detail::{open_album_with, seed_detail_from_settings};
pub use grid::reload_grid;

pub(super) use detail::{open_album, refresh_detail, restore_detail};

/// Installs the grid and detail models and wires every `Albums.*` / `AlbumDetail.*` callback.
///
/// The returned handle is not a keepalive: every wired closure clones its own strong `Arc`, and
/// the `AppWindow` owns those closures for the life of the app.
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

/// The Albums grid's and detail's state, shared by the UI callbacks and the async loads.
pub struct AlbumsUi {
    grid: GridLoads,
    detail: DetailCache<AlbumDetail<'static>>,
    /// The shared row tier, for the detail `TrackList`'s artwork column. The grid's cards draw
    /// from `ui::grid_prewarm::tier()`, which no view holds.
    cover_thumbs: Arc<CoverThumbs>,
    /// The detail header's `(cover, blur)` pair; see [`crate::ui::detail_artwork`].
    detail_artwork: Arc<DetailArtwork>,
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

    /// Drops the detail's `(cover, blur)` pair on close: nothing asks for it before the next
    /// `open_album`, and the grid now on screen stays warm.
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
}

impl_section_state_helpers!(AlbumsUi);

/// Hands both globals their models, each installed once for every album shown and found by
/// downcast afterwards.
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

/// An `AlbumStats` as the `AlbumRow` a card and the detail header render.
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

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<AlbumsUi>();
};
