//! Genres-view glue between Rust and Slint.
//!
//! Two Slint globals are driven from here:
//!
//! * `Genres` — the responsive genre-card grid. Rust owns a flat,
//!   name-sorted `Vec<GenreStats>` (plus pre-lowercased search keys)
//!   behind `GenresUi::grid.data` (fetched once from `genre_stats`); the
//!   grid model is rebuilt from it on every filter / sort / column-count
//!   change *without* a DB hit. The grid is virtualized by row: Rust
//!   chunks the filtered+sorted list into `Genres.columns`-wide rows of
//!   `GenreGridRow`, so the Slint `ListView` only instantiates on-screen
//!   rows.
//!
//! * `GenreDetail` — the full genre detail view. `open_genre` fetches
//!   the header + track list; the cached `Vec<TrackListRow>` in
//!   `GenresUi::detail.tracks` lets `play-row` / `select-row` /
//!   `play-genre` recover ids and re-sort in memory without
//!   round-tripping the Slint model (mirrors `AlbumsUi::detail.tracks`).
//!
//! Unlike Albums / Artists, **no cover or hero-blur caches** live here:
//! genres have no intrinsic artwork. Each grid tile renders the
//! `EntityCard` `theater_comedy` fallback glyph, and the detail header
//! paints only the accent gradient floor + scrim. The only shared cache
//! the Genre tab reads is `cover_thumbs` (row-tier 72 px) for the
//! detail track-list's artwork column — same shared instance Albums /
//! Artists / Tracks use.
//!
//! Cross-thread layout mirrors `tracks.rs` / `albums.rs`: `GenresUi` is
//! `Send + Sync`, cloned into callbacks / tokio tasks; Slint properties
//! and models are only touched from the UI thread, reached via
//! `Weak<AppWindow>::upgrade_in_event_loop`.

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
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::section_state::SectionState;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AppWindow, GenreDetail, GenreGridRow as UiGenreGridRow, GenreRow as UiGenreRow, Genres,
    TrackListRow as UiTrackListRow,
};

use state::{GenreDetailState, GenreGridState, GridData};

#[cfg(test)]
use grid::compute_indices;
#[cfg(test)]
use state::GridIndexCache;

pub use detail::{
    apply_detail_row_favorite, apply_detail_row_rating, apply_filtered_detail, clear_detail,
    open_genre, refresh_detail,
    resort_detail, seed_detail_from_settings, set_filter,
};
pub use grid::{fetch_grid, rebuild_grid};
pub use selection::{clear_selection, handle_select_row};

/// Rust-side state for the Genres grid + detail views. Shared between
/// the UI callbacks (`wire_genres`) and the async fetchers. The grid and
/// detail concerns are largely independent — each lives in its own
/// sub-struct.
///
/// No `grid_covers` / `detail_artwork` caches: genres have no
/// intrinsic image (see module-level doc).
pub struct GenresUi {
    grid: GenreGridState,
    detail: GenreDetailState,
    /// Row-tier (72 px) cache shared with Tracks / Albums / Artists /
    /// now-playing-bar — backs the small artwork column of the detail
    /// view's `TrackList`. Same shared instance the other entity tabs
    /// use; the Genre tab never clears it.
    cover_thumbs: Arc<CoverThumbs>,
    /// Section-visibility + staleness bookkeeping (on-screen shadow, dirty
    /// flag, and the wipe/fetch mutation gate). See [`SectionState`]. Kept
    /// for symmetry with the other entity tabs; no cache-release work is
    /// gated on `section.active()` today (genres carry no artwork to
    /// release), but the hook lives here so a future genre artwork
    /// strategy can wire releases without re-plumbing the callbacks.
    section: SectionState,
}

impl GenresUi {
    pub fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
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
                filter: Mutex::new(String::new()),
            },
            cover_thumbs,
            section: SectionState::new(),
        }
    }

    /// Mirror the Genres-section-visible flag (`section-active-changed`).
    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    /// Whether the Genres section is currently on screen.
    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    /// Hand any retained glibc arena slack back to the OS. Unlike
    /// `AlbumsUi` / `ArtistsUi` there's no LRU to drop here — genres carry
    /// no `grid_covers` / `detail_artwork` (tiles paint procedural
    /// gradients). Used as the post-`clear_detail` close-detail trim
    /// where wiping the grid would be wrong (the user is now looking at
    /// the grid). Section-leave goes through
    /// [`Self::release_section_state`] instead.
    pub fn release_caches(&self) {
        crate::tasks::heap_trim::trim();
    }

    /// Drop *everything* the Genres section is keeping resident — the
    /// canonical grid data (`Vec<GenreStats>` + pre-lowercased keys), the
    /// memoized filter/sort indices, the cached detail track rows, and the
    /// applied-selection shadow — then hand the freed pages back to the
    /// OS. Called (off the UI thread) when the user leaves the Genres
    /// section so the hidden view's resident footprint drops to ~0.
    /// Re-entry re-fetches via [`fetch_grid`]; if a detail was open
    /// (`GenreDetail.genre-id >= 0`), the caller re-runs `open_genre` to
    /// repopulate it. The Slint `Genres.grid-rows` model +
    /// `GenreDetail.{tracks,selected-ids}` are cleared synchronously by
    /// the caller on the UI thread before this runs.
    ///
    /// Race-correct against an in-flight [`fetch_grid`] via the same
    /// early-check + gated-bulk-writes shape as
    /// [`crate::ui::albums::AlbumsUi::release_section_state`] — see that
    /// doc for the full rationale.
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
        crate::tasks::heap_trim::trim();
    }

    /// Mark the cached grid + detail data as stale. See
    /// [`crate::ui::albums::AlbumsUi::mark_dirty`].
    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag. See
    /// [`crate::ui::albums::AlbumsUi::take_dirty`].
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
    }

    /// Genre id currently open in the detail view (`-1` = grid).
    pub fn detail_genre_id(&self) -> i64 {
        *self.detail.genre_id.lock()
    }

    /// Track ids of the **displayed** detail list, in display order — the
    /// filter-applied subset when a search is active, otherwise the full
    /// genre. `play-genre` / shuffle / add-to-queue pass these straight to
    /// `player_play_tracks`, so those actions operate on the visible rows.
    pub fn detail_track_ids(&self) -> Vec<i64> {
        self.detail.tracks.lock().iter().map(|r| r.id).collect()
    }

    /// Surgically flip `is_favorite` on the cached detail row so a
    /// single-row favourite toggle doesn't need a full re-fetch. Both the
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
}

/// Build the empty `VecModel`s the Genres grid, the detail track list,
/// and the detail selection need, and hand them to the Slint globals as
/// `ModelRc`s. Subsequent updates locate them by downcasting back to
/// `VecModel<T>`.
pub fn install_genres_models(ui: &AppWindow) {
    let grid: Rc<VecModel<UiGenreGridRow>> = Rc::new(VecModel::default());
    ui.global::<Genres>().set_grid_rows(ModelRc::from(grid));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<GenreDetail>().set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<GenreDetail>().set_selected_ids(ModelRc::from(sel));
}

/// Convert a `GenreStats` into the Slint `GenreRow` an `EntityCard`
/// (and the detail header) renders. Pure data — there is no cover
/// decode to skip (genres have no artwork), so this stays cheap to
/// run for every genre on every grid rebuild. Hashes the name into a
/// four-stop accent (two tile, two hero) that the Slint side plugs
/// straight into `@linear-gradient` — see [`color::genre_accent`].
pub fn to_slint_genre_row(g: &GenreStats) -> UiGenreRow {
    let accent = color::genre_accent(&g.name);
    UiGenreRow {
        id: clamp_i64_to_i32(g.id),
        name: SharedString::from(g.name.as_str()),
        track_count: g.track_count,
        total_duration_ms: i32::try_from(
            g.total_duration_ms.clamp(0, i64::from(i32::MAX)),
        )
        .unwrap_or(i32::MAX),
        display_duration: SharedString::from(
            crate::ui::tracks::format_duration_ms(g.total_duration_ms),
        ),
        tile_color_1: accent.tile_color_1,
        tile_color_2: accent.tile_color_2,
        hero_color_1: accent.hero_color_1,
        hero_color_2: accent.hero_color_2,
    }
}

#[allow(dead_code)]
fn assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<GenresUi>();
}

#[cfg(test)]
#[path = "tests/genres_tests.rs"]
mod tests;
