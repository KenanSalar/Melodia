//! The Playlists page, shaped like [`crate::ui::albums`]: a card grid, and a detail listing the
//! playlist as a [`TrackRows`] whose keyset, in the playlist's own order, *is* the playlist, so a
//! drag moves one id in it.
//!
//! `Playlists.count` gates every row menu's Add to Playlist, so it is kept current from launch by
//! every playlist write, whatever tab is up.

mod callbacks;
mod detail;
mod grid;

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
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
use melodia_core::entities::playlist::PlaylistStats;
use melodia_core::entities::smart_criteria::Draw;
use melodia_ui::{
    AppWindow, Drag, PlaylistDetail, PlaylistGridRow as UiPlaylistGridRow,
    PlaylistRow as UiPlaylistRow, Playlists,
};

use grid::PlaylistsGrid;

pub use detail::{detail_on_screen, open_playlist_with, seed_detail_from_settings};
pub use grid::reload_grid;

pub(super) use detail::{
    apply_optimistic_move, open_playlist, picked_rows, refresh_detail, refresh_header,
    restore_detail, track_ids_at,
};
pub(super) use grid::{refresh_after_write, refresh_play_stats, shown_card};

/// The M3U8 import and export and the Add-to-Playlist wiring, outside [`install`] for the
/// `Rc<NotificationsUi>` their toasts need, which exists only after the views.
pub use callbacks::wire_files;

/// Installs the grid and detail models and wires every `Playlists.*` / `PlaylistDetail.*`
/// callback but [`wire_files`]'.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<PlaylistsUi> {
    let (grid, detail) = install_models(cx.app, cx.state);
    let playlists_ui = Arc::new(PlaylistsUi::new(
        grid,
        detail,
        cx.cover_thumbs.clone(),
        detail_artwork::blur_spec(cx.app),
        cx.app.global::<Drag>().as_weak(),
    ));
    callbacks::wire(cx.app, cx.state, &playlists_ui);
    // From launch, so the row menus offer Add to Playlist before the tab is ever opened.
    grid::refresh_count(cx.state, &cx.app.as_weak());
    playlists_ui
}

/// The Playlists grid's and detail's state, shared by the UI callbacks and the async loads.
pub struct PlaylistsUi {
    grid: GridLoads,
    detail: DetailCache<PlaylistDetail<'static>>,
    /// The shared row tier, for the detail `TrackList`'s artwork column and the mosaic picker's
    /// tiles. The cards draw from `ui::grid_prewarm::tier()`, which no view holds.
    cover_thumbs: Arc<CoverThumbs>,
    /// The detail header's `(cover, blur)` pair; see [`crate::ui::detail_artwork`].
    detail_artwork: Arc<DetailArtwork>,
    section: SectionState,
    /// The pick a random-capped smart playlist's detail lists, drawn once per open so its sorts,
    /// filters, reloads and hero all describe the same tracks.
    draw: Mutex<Draw>,
    /// What the detail's rows moving revokes, the hook being handed nothing to reach it through.
    drag: slint::Weak<Drag<'static>>,
}

impl PlaylistsUi {
    fn new(
        grid: GridLoads,
        detail: DetailCache<PlaylistDetail<'static>>,
        cover_thumbs: Arc<CoverThumbs>,
        hero_blur: Option<BlurSpec>,
        drag: slint::Weak<Drag<'static>>,
    ) -> Self {
        Self {
            grid,
            detail,
            cover_thumbs,
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
            section: SectionState::new(),
            draw: Mutex::new(Draw::default()),
            drag,
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
        card_grid::prewarm_seeded::<PlaylistsGrid>(self);
    }

    /// The playlist open in the detail, `-1` while the grid is up.
    pub fn detail_playlist_id(&self) -> i64 {
        self.detail.id()
    }

    fn draw(&self) -> Draw {
        *self.draw.lock()
    }

    /// Draws again, for a fresh open.
    fn redraw(&self) {
        *self.draw.lock() = Draw(rand::random());
    }

    /// `Playlists.request-row-cover`, for the mosaic picker's 64 px tiles, off the row tier rather
    /// than a grid-tier decode a tile that size doesn't need.
    pub fn row_cover(&self, artwork_path: &str) -> slint::Image {
        self.cover_thumbs.get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }
}

/// Hands both globals their models, each installed once and found by downcast afterwards.
fn install_models(
    ui: &AppWindow,
    state: &AppState,
) -> (GridLoads, DetailCache<PlaylistDetail<'static>>) {
    let grid = card_grid::install::<PlaylistsGrid>(state, ui);
    let loads = GridLoads::new(grid.scroll_point());
    ui.global::<Playlists>().set_grid_rows(ModelRc::from(grid));

    let g = ui.global::<PlaylistDetail>();
    let tracks: Rc<TrackRows> = TrackRows::new(track_rows::fetcher(state));
    let detail = DetailCache::new(g.as_weak(), tracks.scroll_point());
    g.set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
    (loads, detail)
}

impl GridRow for UiPlaylistGridRow {
    type Card = UiPlaylistRow;

    fn of(playlists: ModelRc<UiPlaylistRow>) -> Self {
        Self { playlists }
    }
}

impl PagedRow for UiPlaylistRow {
    // Blank but live: a card clicked before its fetch lands still opens, plays and picks by id.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }
}

/// A `PlaylistStats` as the `PlaylistRow` a card and the detail header render.
pub fn to_slint_playlist_row(p: &PlaylistStats) -> UiPlaylistRow {
    UiPlaylistRow {
        id: clamp_i64_to_i32(p.id),
        name: SharedString::from(p.name.as_str()),
        description: opt_shared(p.description.as_deref()),
        artwork_path: opt_shared(p.thumbnail_path.as_deref()),
        track_count: p.track_count,
        total_duration_ms: i32::try_from(p.total_duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
        is_smart: p.is_smart,
    }
}

impl_section_state_helpers!(PlaylistsUi);

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<PlaylistsUi>();
};

#[cfg(test)]
#[path = "tests/playlists_tests.rs"]
mod tests;
