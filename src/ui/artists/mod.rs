//! Artists-view glue between Rust and Slint.
//!
//! Two Slint globals are driven from here:
//!
//! * `Artists` — the responsive artist-card grid, the same shape as the Albums grid: Rust owns a
//!   flat, name-sorted `Vec<ArtistStats>` behind `ArtistsUi::grid.data`, and the grid model is
//!   rebuilt from it on every filter / sort / column-count change without a DB hit. Cards are
//!   circular and pull their cover lazily via `request-cover`.
//!
//! * `ArtistDetail` — the full Artist Detail view. `open_artist` fetches the header, the albums
//!   sub-section and the full track list; the cached list in `ArtistsUi::detail.tracks` lets
//!   `play-row`, selection and in-memory re-sort work without round-tripping the Slint model.
//!
//! Cross-thread layout mirrors `albums.rs`: `ArtistsUi` is `Send + Sync`, and Slint properties and
//! models are only touched from the UI thread via `Weak<AppWindow>::upgrade_in_event_loop`.

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

use crate::entities::artist::ArtistStats;
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::ui::albums::AlbumsUi;
use crate::ui::artwork_cache::BlurSpec;
use crate::ui::detail_artwork::{self, DetailArtwork};
use crate::ui::row_match::Needle;
use crate::ui::section_state::{SectionState, impl_detail_row_cache, impl_section_state_helpers};
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, ArtistGridRow as UiArtistGridRow,
    ArtistRow as UiArtistRow, Artists, TrackListRow as UiTrackListRow,
};

use crate::ui::grid_prewarm::GRID_COVER_FALLBACK;
use state::{ArtistDetailState, ArtistGridState, DEFAULT_GRID_COVER_CAP, GridData};

#[cfg(test)]
use grid::compute_indices;
#[cfg(test)]
use state::GridIndexCache;

// Reached from outside the slice: `ui::nav_history` replays a walk into a detail, `boot::ui_setup`
// seeds the persisted one and retunes the cover cap, and its `initial_grid_fetch!` kicks the first
// fetch after the window is shown — which is why `fetch_grid` can't fold into `install`.
pub use detail::{open_artist_with, seed_detail_from_settings};
pub use grid::{fetch_grid, tune_cache_for_display};

// Reached only from this slice's own `callbacks/`, plus the cross-slice `apply_detail_row_*`
// mirrors in `callbacks::now_playing` and the drill in `callbacks::cross_tab_nav`. `pub(super)` is
// `pub(in crate::ui)` here, which is exactly that reach.
pub(super) use detail::{
    apply_detail_row_favorite, apply_detail_row_rating, apply_filtered_detail, clear_detail,
    open_artist, refresh_detail, resort_detail, set_filter,
};
pub(super) use grid::rebuild_grid;
pub(super) use selection::{clear_selection, handle_select_row};

/// Install the Artists grid + detail models, build the handle, and wire every `Artists.*` /
/// `ArtistDetail.*` callback to it.
///
/// Takes `albums_ui` because Artists depends on Albums twice over: the handle borrows the Albums
/// grid-cover tier for its own Albums strip, and the "open album from Artist Detail" hand-off needs
/// a live `AlbumsUi` to call into. That parameter *is* the ordering, so "Albums before Artists" is
/// a compile error to get wrong rather than a comment in the boot file.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>, albums_ui: &Arc<AlbumsUi>) -> Arc<ArtistsUi> {
    install_models(cx.app);
    let artists_ui = Arc::new(ArtistsUi::new(
        cx.cover_thumbs.clone(),
        albums_ui.grid_thumbs(),
        detail_artwork::blur_spec(cx.app),
    ));
    callbacks::wire(cx.app, cx.state, cx.view_state, &artists_ui, albums_ui);
    artists_ui
}

/// Rust-side state for the Artists grid + detail views. Mirrors [`AlbumsUi`] layer by layer; the
/// grid and detail concerns live in their own sub-structs.
pub struct ArtistsUi {
    grid: ArtistGridState,
    detail: ArtistDetailState,
    /// Row-tier cache shared with Tracks / Browse / Albums — backs the small artwork column of the
    /// detail view's `TrackList`.
    cover_thumbs: Arc<CoverThumbs>,
    /// Grid-tier cache for the Artists grid card tiles — private to this view, resolution-derived
    /// cap, released entirely on section leave (see [`Self::release_section_state`]).
    grid_covers: Arc<CoverThumbs>,
    /// Borrowed handle to the **Albums** grid tier. The Artist Detail Albums strip resolves its
    /// cards through `AlbumsUi::grid_cover` (decode-on-miss on the UI thread), so
    /// [`detail::open_artist`]'s fetch prewarms the strip's covers into this cache off-thread
    /// first. Not released here — the Artists wiring already clears the shared LRU through the
    /// `AlbumsUi` handle.
    albums_grid_covers: Arc<CoverThumbs>,
    /// Detail-tier `(cover, blur)` pair cache for the Artist Detail header. Released on section
    /// exit.
    detail_artwork: Arc<DetailArtwork>,
    /// Section-visibility + staleness bookkeeping — see [`SectionState`].
    section: SectionState,
}

impl ArtistsUi {
    fn new(
        cover_thumbs: Arc<CoverThumbs>,
        albums_grid_covers: Arc<CoverThumbs>,
        hero_blur: Option<BlurSpec>,
    ) -> Self {
        Self {
            grid: ArtistGridState {
                data: Mutex::new(Arc::new(GridData::new(Vec::new()))),
                index_cache: Mutex::new(None),
            },
            detail: ArtistDetailState {
                tracks: Mutex::new(Vec::new()),
                all_tracks: Mutex::new(Vec::new()),
                albums: Mutex::new(Vec::new()),
                artist_id: Mutex::new(-1),
                filter: Mutex::new(Needle::default()),
                applied_selection: Mutex::new(HashSet::new()),
            },
            cover_thumbs,
            grid_covers: Arc::new(CoverThumbs::with_config(
                GRID_COVER_FALLBACK,
                DEFAULT_GRID_COVER_CAP,
            )),
            albums_grid_covers,
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
            section: SectionState::new(),
        }
    }

    /// Drop *everything* the Artists section is keeping resident — both cover LRUs, the canonical
    /// grid data, the memoized filter/sort indices, the cached detail track rows and the
    /// applied-selection shadow — then hand the freed pages back to the OS. Called off the UI
    /// thread on section leave. Re-entry re-fetches via [`fetch_grid`]; an open detail is
    /// repopulated by the caller re-running `open_artist`. **The caller clears the Slint-side
    /// properties on the UI thread before this runs.**
    ///
    /// Race-correct against an in-flight [`fetch_grid`] via the same early-check +
    /// gated-bulk-writes shape as [`crate::ui::albums::AlbumsUi::release_section_state`].
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.grid_covers.clear();
        self.detail_artwork.clear();
        {
            let _gate = self.section.gate();
            if self.section_active() {
                return;
            }
            *self.grid.data.lock() = Arc::new(GridData::new(Vec::new()));
            *self.grid.index_cache.lock() = None;
            self.detail.tracks.lock().clear();
            self.detail.all_tracks.lock().clear();
            self.detail.albums.lock().clear();
            self.detail.filter.lock().clear();
            self.detail.applied_selection.lock().clear();
        }
        crate::services::platform::allocator::trim();
    }

    /// Drop just the grid-tier cover cache. Called off the UI thread when the user opens an
    /// artist: the grid view is unmounted by the `ArtistDetail.artist-id` flip.
    pub fn release_grid_covers(&self) {
        self.grid_covers.clear();
        crate::services::platform::allocator::trim();
    }

    /// Drop just the detail-tier `(cover, blur)` pair cache. Called when the user closes a detail.
    pub fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        crate::services::platform::allocator::trim();
    }

    /// Re-decode the first screenful of grid covers into the grid-tier cache after a release.
    pub fn prewarm_visible_covers(&self) {
        let data = self.grid.data.lock().clone();
        let unique = grid::first_screenful_paths(&data);
        if !unique.is_empty() {
            self.grid_covers.prewarm(&unique);
        }
    }

    /// Artist id currently open in the detail view (`-1` = grid).
    pub fn detail_artist_id(&self) -> i64 {
        *self.detail.artist_id.lock()
    }

    /// Lazy cover lookup for an Artists **grid card** — backs `Artists.request-cover`, resolving
    /// against the grid-tier cache.
    pub fn grid_cover(&self, image_path: &str) -> slint::Image {
        self.grid_covers
            .get_or_schedule_opt(crate::ui::grid_prewarm::nonempty_artwork_path(image_path))
    }

    /// The grid tier itself, for the wiring that has to reach past a lookup — the
    /// `AlbumsUi::grid_thumbs` contract.
    pub fn grid_thumbs(&self) -> Arc<CoverThumbs> {
        self.grid_covers.clone()
    }
}

/// Build the empty `VecModel`s the Artists grid, the detail track list, the detail Albums
/// sub-section and the detail selection need, and hand them to the Slint globals as `ModelRc`s.
fn install_models(ui: &AppWindow) {
    let grid: Rc<VecModel<UiArtistGridRow>> = Rc::new(VecModel::default());
    ui.global::<Artists>().set_grid_rows(ModelRc::from(grid));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<ArtistDetail>().set_tracks(ModelRc::from(tracks));

    let albums: Rc<VecModel<UiAlbumRow>> = Rc::new(VecModel::default());
    ui.global::<ArtistDetail>().set_albums(ModelRc::from(albums));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<ArtistDetail>().set_selected_ids(ModelRc::from(sel));
}

/// Convert an `ArtistStats` into the Slint `ArtistRow` an `EntityCard` (and
/// the detail header) renders. Pure data — cover is pulled lazily via
/// `request-cover` so this is cheap on every grid rebuild.
pub fn to_slint_artist_row(a: &ArtistStats) -> UiArtistRow {
    UiArtistRow {
        id: clamp_i64_to_i32(a.id),
        name: SharedString::from(a.name.as_str()),
        image_path: SharedString::from(a.image_path.as_deref().unwrap_or("")),
        track_count: a.track_count,
        album_count: a.album_count,
        total_duration_ms: i32::try_from(a.total_duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
    }
}

impl_section_state_helpers!(ArtistsUi);
impl_detail_row_cache!(ArtistsUi);

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<ArtistsUi>();
};

#[cfg(test)]
#[path = "tests/artists_tests.rs"]
mod tests;
