//! The Artists page, shaped like [`crate::ui::albums`]: a card grid, and a detail whose
//! `open_artist` fetches the header, the Albums strip and the artist's tracks as a [`TrackRows`].

mod callbacks;
mod detail;
mod grid;
mod state;

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, Global as _, ModelRc, SharedString, VecModel};

use crate::ui::albums::AlbumsUi;
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
use melodia_core::entities::artist::ArtistStats;
use melodia_ui::{
    AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, ArtistGridRow as UiArtistGridRow,
    ArtistRow as UiArtistRow, Artists,
};

use grid::ArtistsGrid;
use state::ArtistDetailState;

pub use detail::{open_artist_with, seed_detail_from_settings};
pub use grid::reload_grid;

pub(super) use detail::{open_artist, refresh_detail, restore_detail};

/// Installs the grid and detail models and wires every `Artists.*` / `ArtistDetail.*` callback.
/// `albums_ui` is the open-album hand-off, a parameter so the install order is a compile error.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>, albums_ui: &Arc<AlbumsUi>) -> Arc<ArtistsUi> {
    let (grid, cache) = install_models(cx.app, cx.state);
    let artists_ui = Arc::new(ArtistsUi::new(
        grid,
        cache,
        cx.cover_thumbs.clone(),
        detail_artwork::blur_spec(cx.app),
    ));
    callbacks::wire(cx.app, cx.state, cx.view_state, &artists_ui, albums_ui);
    artists_ui
}

/// The Artists grid's and detail's state, shared by the UI callbacks and the async loads.
pub struct ArtistsUi {
    grid: GridLoads,
    detail: ArtistDetailState,
    /// The shared row tier, for the detail `TrackList`'s artwork column. The cards and the
    /// detail's Albums strip draw from `ui::grid_prewarm::tier()`, which no view holds.
    cover_thumbs: Arc<CoverThumbs>,
    /// The detail header's `(cover, blur)` pair; see [`crate::ui::detail_artwork`].
    detail_artwork: Arc<DetailArtwork>,
    section: SectionState,
}

impl ArtistsUi {
    fn new(
        grid: GridLoads,
        cache: DetailCache<ArtistDetail<'static>>,
        cover_thumbs: Arc<CoverThumbs>,
        hero_blur: Option<BlurSpec>,
    ) -> Self {
        Self {
            grid,
            detail: ArtistDetailState { cache, albums: Mutex::new(Vec::new()) },
            cover_thumbs,
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
            section: SectionState::new(),
        }
    }

    /// Drops the detail's `(cover, blur)` pair on close, for
    /// [`crate::ui::albums::AlbumsUi::release_detail_artwork`]'s reason.
    pub fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        melodia_platform::services::platform::allocator::trim();
    }

    /// Decodes the covers the grid's last load opened on, for a grid coming back from under a
    /// detail. Does nothing unless the grid is what is drawn ([`SectionState::grid_on_screen`]).
    /// Blocking.
    pub fn prewarm_visible_covers(&self) {
        card_grid::prewarm_seeded::<ArtistsGrid>(self);
    }
}

/// Hands both globals their models, each installed once and found by downcast afterwards.
fn install_models(
    ui: &AppWindow,
    state: &AppState,
) -> (GridLoads, DetailCache<ArtistDetail<'static>>) {
    let grid = card_grid::install::<ArtistsGrid>(state, ui);
    let loads = GridLoads::new(grid.scroll_point());
    ui.global::<Artists>().set_grid_rows(ModelRc::from(grid));

    let g = ui.global::<ArtistDetail>();
    let tracks: Rc<TrackRows> = TrackRows::new(track_rows::fetcher(state));
    let cache = DetailCache::new(g.as_weak(), tracks.scroll_point());
    g.set_tracks(ModelRc::from(tracks));

    let albums: Rc<VecModel<UiAlbumRow>> = Rc::new(VecModel::default());
    g.set_albums(ModelRc::from(albums));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
    (loads, cache)
}

impl GridRow for UiArtistGridRow {
    type Card = UiArtistRow;

    fn of(artists: ModelRc<UiArtistRow>) -> Self {
        Self { artists }
    }
}

impl PagedRow for UiArtistRow {
    // Blank but live: a card clicked before its fetch lands still opens, plays and picks by id.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }
}

/// An `ArtistStats` as the `ArtistRow` a card and the detail header render.
pub fn to_slint_artist_row(a: &ArtistStats) -> UiArtistRow {
    UiArtistRow {
        id: clamp_i64_to_i32(a.id),
        name: SharedString::from(a.name.as_str()),
        image_path: opt_shared(a.image_path.as_deref()),
        track_count: a.track_count,
        album_count: a.album_count,
        total_duration_ms: i32::try_from(a.total_duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
    }
}

impl_section_state_helpers!(ArtistsUi);

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<ArtistsUi>();
};
