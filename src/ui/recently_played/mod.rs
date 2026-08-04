//! Recently-Played view glue between Rust and Slint.
//!
//! Drives the `RecentlyPlayed` Slint global — a trimmed cousin of the
//! Favorites page composed of:
//!
//! * A hero — a live 2×2 mosaic of the up-to-4 most-recently-played distinct
//!   covers behind the track count, total duration and the Shuffle / Columns
//!   pill.
//! * A **Most Played** `HorizontalCardStrip` — the library-wide top tracks by
//!   `play_count` (non-collapsible; a small fixed strip).
//! * A filterable `TrackList` bound to the post-filter `RecentlyPlayed.tracks`
//!   model — the 200 most-recently-played tracks (`last_played DESC`). The set
//!   is fetched once per refresh; keystrokes re-walk the cached `tracks_all`
//!   **in memory** (membership is fixed to the 200). The list is mounted
//!   `sortable: false` — recency is the point of the page, so its column
//!   headers resize and toggle but never re-order.
//!
//! Cache discipline mirrors `src/ui/favorites`: the shared row-tier
//! `CoverThumbs` plus one dedicated Most Played tier, released on section
//! leave so the hidden view's resident footprint drops to ~0. Re-enter
//! re-fetches via the `library_changed_tx` / `stats_changed_tx`-driven
//! `mark_dirty` / `take_dirty` round-trip.

mod hero;
mod selection;
mod state;
mod strip;
mod tracks;

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::entities::track::MostPlayedFavorite;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::section_state::SectionState;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AppWindow, EntityStripRow as UiEntityStripRow, RecentlyPlayed,
    TrackListRow as UiTrackListRow,
};

use state::{
    MOSAIC_THUMB_CAP, MOSAIC_THUMB_SIZE, MOST_PLAYED_THUMB_CAP, MOST_PLAYED_THUMB_SIZE,
    RecentlyPlayedUiState,
};

pub use strip::{apply_filtered_strips, refresh_strips};
pub use selection::{clear_selection, handle_select_row};
pub use tracks::{
    apply_filtered_tracks, apply_row_favorite, apply_row_rating, current_filter, refresh_tracks,
    set_filter,
};

/// Rust-side state for the Recently-Played view. Shared between the UI
/// callbacks (`wire_recently_played`) and the async fetchers behind an
/// `Arc<RecentlyPlayedUi>` — `Send + Sync`.
pub struct RecentlyPlayedUi {
    inner: RecentlyPlayedUiState,
    /// Shared row-tier (72 px) cache — used for the `TrackList` row column.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Hero mosaic-tile cache (128 px). Released on section leave.
    pub(super) mosaic_thumbs: Arc<CoverThumbs>,
    /// Most Played strip cache (180 px). Released on section leave.
    pub(super) most_played_thumbs: Arc<CoverThumbs>,
    /// Visibility + staleness + the mutation gate, the same unit the four
    /// entity grids carry. `active` mirrors `Nav.selected-index ==
    /// NAV_RECENTLY_PLAYED && !Nav.now-playing-open`, and gates the refresh
    /// subscriber so a background tick doesn't repaint a hidden view.
    section: SectionState,
}

impl RecentlyPlayedUi {
    pub fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            inner: RecentlyPlayedUiState::new(),
            cover_thumbs,
            mosaic_thumbs: Arc::new(CoverThumbs::with_config(
                MOSAIC_THUMB_SIZE,
                MOSAIC_THUMB_CAP,
            )),
            most_played_thumbs: Arc::new(CoverThumbs::with_config(
                MOST_PLAYED_THUMB_SIZE,
                MOST_PLAYED_THUMB_CAP,
            )),
            section: SectionState::new(),
        }
    }

    pub fn set_section_active(&self, active: bool) {
        self.section.set_active(active);
    }

    pub fn section_active(&self) -> bool {
        self.section.active()
    }

    pub fn mark_dirty(&self) {
        self.section.mark_dirty();
    }

    /// Atomically read-and-clear the dirty flag.
    pub fn take_dirty(&self) -> bool {
        self.section.take_dirty()
    }

    /// Serialize a bulk-state wipe against a data write. Held only around
    /// the write; never across an `.await`.
    pub(super) fn gate(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.section.gate()
    }

    /// Forget the mosaic recorded as being on screen, so the next refresh
    /// recomposes the hero blur. Paired with the leave handler's `blur-img-*`
    /// wipe rather than with [`Self::release_section_state`] — see
    /// [`crate::ui::favorites::FavoritesUi::forget_mosaic`], including for why
    /// this is the guard's only mover outside `hero.rs`.
    pub fn forget_mosaic(&self) {
        self.inner.last_mosaic_paths.lock().clear();
    }

    /// Drop every section-local resident buffer so the hidden view's
    /// footprint drops to ~0. Called (off the UI thread) on section leave;
    /// `mark_dirty()` was set synchronously on the same leave so the
    /// section-enter handler re-fetches via `take_dirty()`. Release ordering
    /// matches `FavoritesUi::release_section_state`, gate included: the state
    /// wipe is serialized against a `refresh_strips` / `refresh_tracks` store
    /// so neither can land halfway through the other.
    ///
    /// The gate serializes those writes; it does **not** order them, so it is
    /// not what stops a fetch that resolves *after* the leave from repopulating
    /// what this just emptied. That is each fetcher's own `section_active()`
    /// bail, plus the one inside each apply's event-loop closure — the pair
    /// `favorites::grids` carries, and the reason the wipe can be unconditional
    /// below the early return.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.mosaic_thumbs.clear();
        self.most_played_thumbs.clear();
        {
            let _gate = self.gate();
            self.inner.tracks_all.lock().clear();
            self.inner.most_played.lock().clear();
            self.inner.applied_selection.lock().clear();
        }
        crate::tasks::heap_trim::trim();
    }

    pub(crate) fn state(&self) -> &RecentlyPlayedUiState {
        &self.inner
    }

    /// Lazy cover lookup for the hero 2×2 mosaic tiles. Routed via
    /// `RecentlyPlayed.request-mosaic-cover`.
    pub fn mosaic_cover(&self, artwork_path: &str) -> Image {
        self.mosaic_thumbs
            .get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Lazy cover lookup for the Most Played strip cards. Routed via
    /// `RecentlyPlayed.request-most-played-cover`.
    pub fn most_played_cover(&self, artwork_path: &str) -> Image {
        self.most_played_thumbs
            .get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Track ids of the Most Played strip, in card order. `play-track` hands
    /// these to `player_play_tracks` so clicking a card loads the strip rather
    /// than the recency list below it.
    ///
    /// Filtered through the same predicate `apply_filtered_strips` builds the
    /// model with — the strip narrows with the search bar, so the raw cache
    /// would enqueue cards that aren't on screen.
    pub fn most_played_track_ids(&self) -> Vec<i64> {
        let needle = self.inner.filter.lock().clone();
        self.inner
            .most_played
            .lock()
            .iter()
            .filter(|t| crate::ui::row_match::most_played_matches(t, &needle))
            .map(|t| t.id)
            .collect()
    }

    /// Track ids of the post-filter list in **display order** — recency, less
    /// whatever the search bar has narrowed away — so `shuffle-all` /
    /// `play-row` enqueue what the user sees.
    pub fn filtered_track_ids(&self) -> Vec<i64> {
        let needle = self.inner.filter.lock().clone();
        self.inner
            .tracks_all
            .lock()
            .iter()
            .filter(|r| crate::ui::row_match::track_matches(r, &needle))
            .map(|r| r.id)
            .collect()
    }

    /// Surgically flip `is_favorite` on a cached row so a single-row toggle
    /// reflects in the model without a full re-fetch. Unlike Favorites this
    /// never removes the row — recency membership is independent of the
    /// favorite flag.
    pub fn flip_track_favorite(&self, id: i64, fav: bool) {
        if let Some(r) = self.inner.tracks_all.lock().iter_mut().find(|r| r.id == id) {
            r.is_favorite = fav;
        }
    }

    /// Surgically set `rating` on a cached row — the star-rating analogue of
    /// [`Self::flip_track_favorite`]. Recency membership is fixed to the 200
    /// most-recent rows, so rating never removes the row.
    pub fn flip_track_rating(&self, id: i64, rating: i32) {
        if let Some(r) = self.inner.tracks_all.lock().iter_mut().find(|r| r.id == id) {
            r.rating = rating;
        }
    }
}

/// Bind empty Slint `VecModel`s for the Most Played strip, the track list, and
/// the selection set. Subsequent updates locate them by downcasting back to
/// `VecModel<T>` from the UI thread.
pub fn install_recently_played_models(ui: &AppWindow) {
    let g = ui.global::<RecentlyPlayed>();

    let mosaic_paths: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
    g.set_mosaic_paths(ModelRc::from(mosaic_paths));

    let most_played: Rc<VecModel<UiEntityStripRow>> = Rc::new(VecModel::default());
    g.set_most_played_rows(ModelRc::from(most_played));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    g.set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
}

/// Map a `MostPlayedFavorite` to its Slint strip row. Subtitle is the artist
/// name; `play_count` rides in the `play_count` slot so the strip's
/// `show-play-count: true` reveals it.
pub fn to_slint_most_played_row(t: &MostPlayedFavorite) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(t.id),
        title: SharedString::from(t.title.as_str()),
        subtitle: SharedString::from(t.artist.as_deref().unwrap_or("")),
        artwork_path: SharedString::from(t.artwork_path.as_deref().unwrap_or("")),
        play_count: t.play_count,
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<RecentlyPlayedUi>();
};

#[cfg(test)]
#[path = "tests/recently_played_tests.rs"]
mod tests;
