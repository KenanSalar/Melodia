//! The Genres page: a virtualized card grid, and the detail view a card drills
//! into. Shaped like [`crate::ui::albums`], whose module docs argue the paged
//! grid and the detail's keyset.
//!
//! What differs is that **no cover or hero-blur caches live here** — genres have
//! no intrinsic artwork. Each tile renders `EntityCard`'s fallback glyph, and
//! the detail header paints only its accent gradient and scrim. The one shared
//! cache this page reads is the row tier, for the detail `TrackList`'s artwork
//! column.

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

// Re-exported for the Search view's Top Result card: a genre shown there
// has to carry the same tint as its card in this grid, and the only way
// to guarantee that is to derive both from the one function.
pub use color::{GenreAccent, genre_accent};
// Reached from outside the slice: `boot::ui_setup` seeds the persisted detail,
// `ui::nav_history` replays a walk into one, and the cross-tab drill lands here
// from `callbacks::cross_tab_nav`.
pub use detail::{open_genre_with, seed_detail_from_settings};
// `boot::ui_setup`'s `initial_grid_fetch!` kicks this after the window is shown,
// which is why it can't fold into `install` with the models and the wiring.
pub use grid::reload_grid;

// `pub(super)` is `pub(in crate::ui)` here, the reach this slice's own `callbacks/` and the drill
// in `callbacks::cross_tab_nav` need.
pub(super) use detail::{open_genre, refresh_detail, restore_detail};

/// Install the Genres grid + detail models, build the handle, and wire every
/// `Genres.*` / `GenreDetail.*` callback to it.
///
/// Self-contained: no cross-tab origin and no artwork, just the shared row tier
/// for the detail list's artwork column.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<GenresUi> {
    let (grid, detail) = install_models(cx.app, cx.state);
    let genres_ui = Arc::new(GenresUi::new(grid, detail, cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, cx.view_state, &genres_ui);
    genres_ui
}

/// Rust-side state for the Genres grid and detail, shared between the UI
/// callbacks and the async loads — and there is no cover tier at all, this
/// page having no artwork.
pub struct GenresUi {
    grid: GridLoads,
    detail: DetailCache<GenreDetail<'static>>,
    /// The shared row tier, for the detail `TrackList`'s artwork column. This
    /// page never clears it.
    cover_thumbs: Arc<CoverThumbs>,
    /// Visibility + staleness. See [`SectionState`].
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

    /// Hand retained glibc arena slack back, with no LRU to drop first. The
    /// close-detail trim, where wiping the grid would be wrong — the user is now
    /// looking at it. A section leave takes [`Self::release_section_state`].
    pub fn release_caches(&self) {
        melodia_platform::services::platform::allocator::trim();
    }

    /// Hand the freed pages back on section leave, off the UI thread, the caller having released
    /// the grid and the detail's rows there already. Re-entry reloads the grid, and re-runs
    /// `open_genre` if a detail was open.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        melodia_platform::services::platform::allocator::trim();
    }

    /// Genre id currently open in the detail view (`-1` = grid).
    pub fn detail_genre_id(&self) -> i64 {
        self.detail.id()
    }
}

/// Hand the two globals their empty models, each installed once: the grid a `PagedGrid`, the
/// detail's tracks a [`TrackRows`] for every genre it will show. Later updates find them by
/// downcasting back.
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

/// A `GenreStats` as the `GenreRow` an `EntityCard` and the detail header
/// render. Cheap: there is no cover to decode. The name hashes into the tile stops the Slint side plugs
/// into `@linear-gradient`; the dimmer backdrop pair stays on the Rust side,
/// which is the only place it is read. See [`color::genre_accent`].
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

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<GenresUi>();
};
