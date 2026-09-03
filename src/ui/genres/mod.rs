//! The Genres page: a virtualized card grid, and the detail view a card drills
//! into. Shaped like [`crate::ui::albums`], whose module docs argue the grid
//! data and the cached detail rows.
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
mod selection;
mod state;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::genre::GenreStats;
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::ui::row_match::Needle;
use crate::ui::section_state::{SectionState, impl_detail_row_cache, impl_section_state_helpers};
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AppWindow, GenreDetail, GenreGridRow as UiGenreGridRow, GenreRow as UiGenreRow, Genres,
    TrackListRow as UiTrackListRow,
};

use state::{GenreDetailState, GenreGridState, GridData};

#[cfg(test)]
use grid::compute_indices;
#[cfg(test)]
use state::GridIndexCache;

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
pub use grid::fetch_grid;

// `pub(super)` is `pub(in crate::ui)` here — one notch wider than this slice's
// own `callbacks/` needs, and the notch the cross-slice `apply_detail_row_*`
// mirrors in `callbacks::now_playing` still use.
pub(super) use detail::{
    apply_detail_row_favorite, apply_detail_row_rating, apply_filtered_detail, clear_detail,
    open_genre, refresh_detail, resort_detail, set_filter,
};
pub(super) use grid::rebuild_grid;
pub(super) use selection::{clear_selection, handle_select_row};

/// Install the Genres grid + detail models, build the handle, and wire every
/// `Genres.*` / `GenreDetail.*` callback to it.
///
/// Self-contained: no cross-tab origin and no artwork, just the shared row tier
/// for the detail list's artwork column.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<GenresUi> {
    install_models(cx.app);
    let genres_ui = Arc::new(GenresUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, cx.view_state, &genres_ui);
    genres_ui
}

/// Rust-side state for the Genres grid and detail, shared between the UI
/// callbacks and the async fetchers. The two concerns are largely independent,
/// so each lives in its own sub-struct — and there is no cover tier at all, this
/// page having no artwork.
pub struct GenresUi {
    grid: GenreGridState,
    detail: GenreDetailState,
    /// The shared row tier, for the detail `TrackList`'s artwork column. This
    /// page never clears it.
    cover_thumbs: Arc<CoverThumbs>,
    /// Visibility + staleness + the mutation gate. See [`SectionState`]. Kept
    /// for symmetry with the other entity tabs: nothing is gated on it today,
    /// there being no artwork to release, but the hook is here so a future
    /// genre-artwork strategy needn't re-plumb the callbacks.
    section: SectionState,
}

impl GenresUi {
    fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            grid: GenreGridState {
                data: Mutex::new(Arc::new(GridData::new(Vec::new()))),
                index_cache: Mutex::new(None),
            },
            detail: GenreDetailState {
                tracks: Mutex::new(Vec::new()),
                all_tracks: Mutex::new(Vec::new()),
                genre_id: Mutex::new(-1),
                applied_selection: Mutex::new(HashSet::new()),
                filter: Mutex::new(Needle::default()),
            },
            cover_thumbs,
            section: SectionState::new(),
        }
    }

    /// Hand retained glibc arena slack back, with no LRU to drop first. The
    /// close-detail trim, where wiping the grid would be wrong — the user is now
    /// looking at it. A section leave takes [`Self::release_section_state`].
    pub fn release_caches(&self) {
        crate::services::allocator::trim();
    }

    /// Drop *everything* the section keeps resident — the canonical grid data,
    /// the memoized indices, the cached detail rows and the selection shadow —
    /// then hand the freed pages back. Runs off the UI thread on section leave;
    /// the caller has already cleared the Slint models there. Re-entry
    /// re-fetches through [`fetch_grid`], and re-runs `open_genre` if a detail
    /// was open.
    ///
    /// Race-correct against an in-flight [`fetch_grid`] through the same
    /// early-check plus gated-writes shape as
    /// [`crate::ui::albums::AlbumsUi::release_section_state`], which argues it.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        {
            let _gate = self.section.gate();
            if self.section_active() {
                return;
            }
            *self.grid.data.lock() = Arc::new(GridData::new(Vec::new()));
            *self.grid.index_cache.lock() = None;
            self.detail.tracks.lock().clear();
            self.detail.all_tracks.lock().clear();
            self.detail.applied_selection.lock().clear();
        }
        crate::services::allocator::trim();
    }

    /// Genre id currently open in the detail view (`-1` = grid).
    pub fn detail_genre_id(&self) -> i64 {
        *self.detail.genre_id.lock()
    }
}

/// Hand the two globals their empty `VecModel`s. Later updates find them by
/// downcasting back.
fn install_models(ui: &AppWindow) {
    let grid: Rc<VecModel<UiGenreGridRow>> = Rc::new(VecModel::default());
    ui.global::<Genres>().set_grid_rows(ModelRc::from(grid));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<GenreDetail>().set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<GenreDetail>().set_selected_ids(ModelRc::from(sel));
}

/// A `GenreStats` as the `GenreRow` an `EntityCard` and the detail header
/// render. Cheap enough to run for every genre on every rebuild — there is no
/// cover to decode. The name hashes into the tile stops the Slint side plugs
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
impl_detail_row_cache!(GenresUi);

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<GenresUi>();
};

#[cfg(test)]
#[path = "tests/genres_tests.rs"]
mod tests;
