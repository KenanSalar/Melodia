//! The Genres page, shaped like [`crate::ui::albums`] less every cover: a genre has no artwork,
//! so its card and detail header paint a gradient hashed off the name ([`color::genre_accent`]).

mod callbacks;
mod color;
mod detail;
mod grid;

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Global as _, ModelRc, SharedString, VecModel};

use crate::ui::card_grid::{self, GridLoads};
use crate::ui::paged_rows::PagedRow;
use crate::ui::paged_rows::grid::GridRow;
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::track_detail::DetailCache;
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::genre::GenreStats;
use melodia_ui::{
    AppWindow, GenreDetail, GenreGridRow as UiGenreGridRow, GenreRow as UiGenreRow, Genres,
};

use grid::GenresGrid;

// Search's Top Result tints a genre from the same function, so it matches the card here.
pub use color::{GenreAccent, genre_accent};
pub use detail::{open_genre_with, seed_detail_from_settings};
pub use grid::reload_grid;

pub(super) use detail::{open_genre, refresh_detail, restore_detail};

/// Installs the grid and detail models and wires every `Genres.*` / `GenreDetail.*` callback.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<GenresUi> {
    let (grid, detail) = install_models(cx.app, cx.state);
    let genres_ui = Arc::new(GenresUi::new(grid, detail, cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, cx.view_state, &genres_ui);
    genres_ui
}

/// The Genres grid's and detail's state, shared by the UI callbacks and the async loads.
pub struct GenresUi {
    grid: GridLoads,
    detail: DetailCache<GenreDetail<'static>>,
    /// The shared row tier, for the detail `TrackList`'s artwork column.
    cover_thumbs: Arc<CoverThumbs>,
    section: SectionState,
}

impl GenresUi {
    fn new(
        grid: GridLoads,
        detail: DetailCache<GenreDetail<'static>>,
        cover_thumbs: Arc<CoverThumbs>,
    ) -> Self {
        Self { grid, detail, cover_thumbs, section: SectionState::new() }
    }

    /// The close-detail trim, with no cover to drop first and the grid now on screen left alone.
    /// A section leave trims through `callbacks::grid_tab` instead.
    pub fn release_caches(&self) {
        melodia_platform::services::platform::allocator::trim();
    }
}

/// Hands both globals their models, each installed once and found by downcast afterwards.
fn install_models(
    ui: &AppWindow,
    state: &AppState,
) -> (GridLoads, DetailCache<GenreDetail<'static>>) {
    let grid = card_grid::install::<GenresGrid>(state, ui);
    let loads = GridLoads::new(grid.scroll_point());
    ui.global::<Genres>().set_grid_rows(ModelRc::from(grid));

    let g = ui.global::<GenreDetail>();
    let tracks: Rc<TrackRows> = TrackRows::new(track_rows::fetcher(state));
    let detail = DetailCache::new(g.as_weak(), tracks.scroll_point());
    g.set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
    (loads, detail)
}

impl GridRow for UiGenreGridRow {
    type Card = UiGenreRow;

    fn of(genres: ModelRc<UiGenreRow>) -> Self {
        Self { genres }
    }
}

impl PagedRow for UiGenreRow {
    // Blank but live: a card clicked before its fetch lands still opens, plays and picks by id.
    fn placeholder(id: i64) -> Self {
        Self { id: clamp_i64_to_i32(id), ..Self::default() }
    }
}

/// A `GenreStats` as the `GenreRow` a card and the detail header render, carrying the tile stops
/// of [`color::genre_accent`]; the dimmer hero pair is read only in Rust and stays there.
pub fn to_slint_genre_row(g: &GenreStats) -> UiGenreRow {
    let accent = color::genre_accent(&g.name);
    UiGenreRow {
        id: clamp_i64_to_i32(g.id),
        name: SharedString::from(g.name.as_str()),
        track_count: g.track_count,
        total_duration_ms: i32::try_from(g.total_duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
        display_duration: SharedString::from(crate::ui::tracks::format_duration_ms(
            g.total_duration_ms,
        )),
        tile_color_1: accent.tile_color_1,
        tile_color_2: accent.tile_color_2,
    }
}

impl_section_state_helpers!(GenresUi);

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<GenresUi>();
};
