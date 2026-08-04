//! Artists-view glue between Rust and Slint.
//!
//! Two Slint globals are driven from here:
//!
//! * `Artists` — the responsive artist-card grid. Same shape as the Albums
//!   grid: Rust owns a flat, name-sorted `Vec<ArtistStats>` (plus a pre-
//!   lowercased sort key) behind `ArtistsUi::grid.data`; the grid model
//!   is rebuilt from it on every filter / sort / column-count change
//!   without a DB hit. Cards are circular (Tauri parity) and pull their
//!   cover lazily via `request-cover`, exactly like the Album cards do.
//!
//! * `ArtistDetail` — the full Artist Detail view. `open_artist` fetches
//!   the header + the artist's albums sub-section + the full track list.
//!   The cached track list in `ArtistsUi::detail.tracks` lets `play-row`,
//!   selection, and in-memory re-sort work without round-tripping the
//!   Slint model.
//!
//! Cross-thread layout mirrors `albums.rs`: `ArtistsUi` is `Send + Sync`;
//! Slint properties + models are only touched from the UI thread, reached
//! via `Weak<AppWindow>::upgrade_in_event_loop`.

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
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::detail_artwork::DetailArtwork;
use crate::ui::row_match::Needle;
use crate::ui::section_state::SectionState;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, ArtistGridRow as UiArtistGridRow,
    ArtistRow as UiArtistRow, Artists, TrackListRow as UiTrackListRow,
};

use state::{
    ArtistDetailState, ArtistGridState, DEFAULT_GRID_COVER_CAP, GRID_COVER_SIZE, GridData,
};

#[cfg(test)]
use grid::compute_indices;
#[cfg(test)]
use state::GridIndexCache;

pub use detail::{
    apply_detail_row_favorite, apply_detail_row_rating, apply_filtered_detail, clear_detail,
    open_artist, open_artist_with,
    refresh_detail, resort_detail, seed_detail_from_settings, set_filter,
};
pub use grid::{fetch_grid, rebuild_grid, tune_cache_for_display};
pub use selection::{clear_selection, handle_select_row};

/// Rust-side state for the Artists grid + detail views. Mirrors
/// [`AlbumsUi`](crate::ui::albums::AlbumsUi) layer by layer; the grid and
/// detail concerns live in their own sub-structs.
pub struct ArtistsUi {
    grid: ArtistGridState,
    detail: ArtistDetailState,
    /// Row-tier (72 px) cache shared with Tracks / Browse / Albums — backs
    /// the small artwork column of the detail view's `TrackList`.
    cover_thumbs: Arc<CoverThumbs>,
    /// Grid-tier (`GRID_COVER_SIZE`) cache for the Artists grid card tiles
    /// — private to this view, resolution-derived cap. Released entirely
    /// (see [`Self::release_section_state`]) whenever the user leaves the
    /// Artists section, and re-warmed on return.
    grid_covers: Arc<CoverThumbs>,
    /// Borrowed handle to the **Albums** grid tier
    /// ([`crate::ui::albums::AlbumsUi::grid_thumbs`]). The Artist Detail
    /// Albums strip resolves its cards through `AlbumsUi::grid_cover`
    /// (decode-on-miss on the UI thread), so [`detail::open_artist`]'s
    /// fetch prewarms the strip's covers into this cache off-thread first.
    /// Not released here — `wire_artists` already clears the shared LRU on
    /// section-leave / detail-close via the `AlbumsUi` handle.
    albums_grid_covers: Arc<CoverThumbs>,
    /// Detail-tier `(cover, blur)` pair cache for the Artist Detail header.
    /// Released on section exit.
    detail_artwork: Arc<DetailArtwork>,
    /// Section-visibility + staleness bookkeeping (on-screen shadow, dirty
    /// flag, and the wipe/fetch mutation gate). See [`SectionState`].
    section: SectionState,
}

impl ArtistsUi {
    pub fn new(cover_thumbs: Arc<CoverThumbs>, albums_grid_covers: Arc<CoverThumbs>) -> Self {
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
                GRID_COVER_SIZE,
                DEFAULT_GRID_COVER_CAP,
            )),
            albums_grid_covers,
            detail_artwork: Arc::new(DetailArtwork::new()),
            section: SectionState::new(),
        }
    }

    /// Mirror the Artists-section-visible flag (`section-active-changed`).
    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    /// Whether the Artists section is currently on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Drop *everything* the Artists section is keeping resident — both
    /// cover LRUs, the canonical grid data (`Vec<ArtistStats>` +
    /// pre-lowercased keys), the memoized filter/sort indices, the cached
    /// detail track rows, and the applied-selection shadow — then hand the
    /// freed pages back to the OS. Called (off the UI thread) when the user
    /// leaves the Artists section so the hidden view's resident footprint
    /// drops to ~0. Re-entry re-fetches via [`fetch_grid`]; if a detail was
    /// open (`ArtistDetail.artist-id >= 0`), the caller re-runs
    /// `open_artist` to repopulate it. The caller is responsible for
    /// clearing the Slint-side properties (`Artists.grid-rows`, the
    /// `ArtistDetail` cover / blur Images, and the tracks / albums /
    /// selected-ids `VecModel`s) on the UI thread before this runs.
    ///
    /// Race-correct against an in-flight [`fetch_grid`] via the same
    /// early-check + gated-bulk-writes shape as
    /// [`crate::ui::albums::AlbumsUi::release_section_state`] — see that
    /// doc for the full rationale.
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
        crate::tasks::heap_trim::trim();
    }

    /// Mark the cached grid + detail data as "must be re-fetched on the
    /// next section-enter". Synchronous on the UI thread; see
    /// [`crate::ui::albums::AlbumsUi::mark_dirty`] for the race rationale.
    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag. See
    /// [`crate::ui::albums::AlbumsUi::take_dirty`].
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
    }

    /// Drop just the grid-tier cover cache. Called (off the UI thread)
    /// when the user opens an artist: the grid view is unmounted by the
    /// `ArtistDetail.artist-id` flip.
    pub fn release_grid_covers(&self) {
        self.grid_covers.clear();
        crate::tasks::heap_trim::trim();
    }

    /// Drop just the detail-tier `(cover, blur)` pair cache. Called when
    /// the user closes a detail view.
    pub fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        crate::tasks::heap_trim::trim();
    }

    /// Re-decode the first screenful of grid covers into the grid-tier
    /// cache after a release.
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

    /// Track ids of the **displayed** detail list, in display order — the
    /// filter-applied subset when a search is active, otherwise every
    /// track. Play / shuffle / add-to-queue act on exactly these rows.
    pub fn detail_track_ids(&self) -> Vec<i64> {
        self.detail.tracks.lock().iter().map(|r| r.id).collect()
    }

    /// Surgically flip `is_favorite` on the cached detail row. Both the
    /// displayed `tracks` cache and the canonical `all_tracks` set are
    /// touched so a later `apply_filtered_detail` rebuild keeps the star.
    pub fn flip_detail_favorite(&self, id: i64, fav: bool) {
        if let Some(r) = self.detail.tracks.lock().iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
        if let Some(r) = self.detail.all_tracks.lock().iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
    }

    /// Star-rating analogue of [`Self::flip_detail_favorite`] — set `rating`
    /// on both the displayed `tracks` cache and the canonical `all_tracks` set.
    pub fn flip_detail_rating(&self, id: i64, rating: i32) {
        if let Some(r) = self.detail.tracks.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
        if let Some(r) = self.detail.all_tracks.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
    }

    /// Lazy cover lookup for an Artists **grid card** — backs
    /// `Artists.request-cover`. Resolves against the grid-tier
    /// (`GRID_COVER_SIZE`) cache.
    pub fn grid_cover(&self, image_path: &str) -> slint::Image {
        self.grid_covers
            .get_or_load_opt(Some(image_path).filter(|s| !s.is_empty()))
    }
}

/// Build the empty `VecModel`s the Artists grid, the detail track list,
/// the detail Albums sub-section, and the detail selection need, and hand
/// them to the Slint globals as `ModelRc`s.
pub fn install_artists_models(ui: &AppWindow) {
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
        total_duration_ms: i32::try_from(
            a.total_duration_ms.clamp(0, i64::from(i32::MAX)),
        )
        .unwrap_or(i32::MAX),
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<ArtistsUi>();
};

#[cfg(test)]
#[path = "tests/artists_tests.rs"]
mod tests;
