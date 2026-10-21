//! Artists-view glue between Rust and Slint.
//!
//! Two Slint globals are driven from here:
//!
//! * `Artists` — the responsive artist-card grid, the same shape as the Albums grid: a paged grid
//!   of the artists' ids in the current sort and filter, its loads `ui::card_grid`'s. Cards are
//!   circular and pull their cover lazily via `request-cover`.
//!
//! * `ArtistDetail` — the full Artist Detail view. `open_artist` fetches the header, the albums
//!   sub-section and the artist's tracks as a keyset in a [`TrackRows`], whose [`DetailCache`]
//!   holds the sort and filter its loads run under.
//!
//! Cross-thread layout mirrors `albums.rs`: `ArtistsUi` is `Send + Sync`, and Slint properties and
//! models are only touched from the UI thread via `Weak<AppWindow>::upgrade_in_event_loop`.

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

// Reached from outside the slice: `ui::nav_history` replays a walk into a detail, `boot::ui_setup`
// seeds the persisted one, and its `initial_grid_fetch!` kicks the first load after the window is
// shown — which is why `reload_grid` can't fold into `install`.
pub use detail::{open_artist_with, seed_detail_from_settings};
pub use grid::reload_grid;

// Reached from this slice's own `callbacks/` and the drill in `callbacks::cross_tab_nav`.
// `pub(super)` is `pub(in crate::ui)` here, which is exactly that reach.
pub(super) use detail::{open_artist, refresh_detail, restore_detail};

/// Install the Artists grid + detail models, build the handle, and wire every `Artists.*` /
/// `ArtistDetail.*` callback to it.
///
/// Takes `albums_ui` for the "open album from Artist Detail" hand-off, which needs a live
/// `AlbumsUi` to call into. That parameter *is* the ordering, so "Albums before Artists" is a
/// compile error to get wrong rather than a comment in the boot file. The detail's Albums strip
/// no longer needs it — one tier serves every grid, so there is nothing left to borrow.
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

/// Rust-side state for the Artists grid + detail views. Mirrors [`AlbumsUi`] layer by layer; the
/// grid and detail concerns live in their own sub-structs.
pub struct ArtistsUi {
    grid: GridLoads,
    detail: ArtistDetailState,
    /// Row-tier cache shared with Tracks / Browse / Albums — backs the small artwork column of the
    /// detail view's `TrackList`. The grid cards and the detail's Albums strip both draw from
    /// `ui::grid_prewarm::tier()`, which is why neither has a field here — one tier serves them
    /// and the strip has nothing to borrow.
    cover_thumbs: Arc<CoverThumbs>,
    /// Detail-tier `(cover, blur)` pair cache for the Artist Detail header. Released on section
    /// exit.
    detail_artwork: Arc<DetailArtwork>,
    /// Section-visibility + staleness bookkeeping — see [`SectionState`].
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

    /// Drop just the detail-tier `(cover, blur)` pair cache. Called when the user closes a detail.
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

/// Build the empty models the Artists grid, the detail track list, the detail Albums sub-section
/// and the detail selection need, and hand them to the Slint globals as `ModelRc`s. The grid is a
/// `PagedGrid` and the track list a [`TrackRows`], each installed once.
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

/// Convert an `ArtistStats` into the Slint `ArtistRow` an `EntityCard` (and
/// the detail header) renders. Pure data — cover is pulled lazily via
/// `request-cover`.
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

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<ArtistsUi>();
};
