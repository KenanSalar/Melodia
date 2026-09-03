//! Playlists-view glue between Rust and Slint.
//!
//! Two Slint globals are driven from here:
//!
//! * `Playlists` — the responsive playlist-card grid. Rust owns a flat
//!   `Vec<PlaylistStats>` (plus a pre-lowercased name sort key) behind
//!   `PlaylistsUi::grid.data`; the grid model is rebuilt from it on every
//!   filter / sort / column-count change *without* a DB hit. Per-card cover
//!   lookup is lazy via `request-cover`. The flat `Playlists.rows` model
//!   (consumed by the per-track Add-to-Playlist submenu) is kept in
//!   `updated_at DESC` order and rebuilt on every `fetch_grid`.
//!
//! * `PlaylistDetail` — the full playlist detail view. `open_playlist`
//!   fetches the header + track list; the cached `Vec<TrackListRow>` in
//!   `PlaylistsUi::detail.tracks` (mirroring `AlbumsUi::detail`) plus the
//!   canonical position-order id list let `play-row`, `select-row`,
//!   `resort_detail`, drag-reorder, and the `"position"` sort recover
//!   indices without round-tripping the Slint model.
//!
//! Cross-thread layout mirrors `albums.rs`: `PlaylistsUi` is `Send + Sync`,
//! cloned into callbacks / tokio tasks; Slint properties and models are
//! only touched from the UI thread via
//! `Weak<AppWindow>::upgrade_in_event_loop`.

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

use crate::entities::playlist::PlaylistStats;
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::ui::artwork_cache::BlurSpec;
use crate::ui::detail_artwork::{self, DetailArtwork};
use crate::ui::row_match::Needle;
use crate::ui::section_state::{SectionState, impl_detail_row_cache, impl_section_state_helpers};
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AppWindow, PlaylistDetail, PlaylistGridRow as UiPlaylistGridRow, PlaylistRow as UiPlaylistRow,
    Playlists, TrackListRow as UiTrackListRow,
};

use crate::ui::grid_prewarm::GRID_COVER_FALLBACK;
use state::{DEFAULT_GRID_COVER_CAP, GridData, PlaylistDetailState, PlaylistGridState};

#[cfg(test)]
use grid::compute_indices;

// Reached from outside the slice: `ui::nav_history` replays a walk into a
// detail, `boot::ui_setup` seeds the persisted one and retunes the cover cap,
// and its `initial_grid_fetch!` kicks the first fetch after the window is shown
// — which is why `fetch_grid` can't fold into `install` with the rest.
pub use detail::{open_playlist_with, seed_detail_from_settings};
pub use grid::{fetch_grid, tune_cache_for_display};

// Reached only from this slice's own `callbacks/`, which used to live two
// modules away, plus the cross-slice `apply_detail_row_*` mirrors in
// `callbacks::now_playing`. `pub(super)` is `pub(in crate::ui)` here, which is
// exactly that reach.
pub(super) use detail::{
    POSITION_FIELD, apply_detail_row_favorite, apply_detail_row_rating, apply_filtered_detail,
    apply_optimistic_reorder, clear_detail, open_playlist, refresh_detail, resort_detail,
    rollback_reorder, set_filter,
};
pub(super) use grid::{fetch_grid_stats, rebuild_grid};
pub(super) use selection::{clear_selection, handle_select_row};

/// The M3U8 import / export wiring, kept out of [`install`] because it needs
/// the `Rc<NotificationsUi>` for its completion toasts and that is created
/// after the per-view wiring runs. `main.rs` calls it once the stack exists.
pub use callbacks::wire_files;

/// Install the Playlists grid + detail models, build the handle, and wire every
/// `Playlists.*` / `PlaylistDetail.*` callback to it — except the file
/// import/export pair, which is [`wire_files`].
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<PlaylistsUi> {
    install_models(cx.app);
    let playlists_ui =
        Arc::new(PlaylistsUi::new(cx.cover_thumbs.clone(), detail_artwork::blur_spec(cx.app)));
    callbacks::wire(cx.app, cx.state, &playlists_ui);
    playlists_ui
}

/// Rust-side state for the Playlists grid + detail views. Shared between
/// the UI callbacks (`callbacks::wire`) and the async fetchers. Mirrors
/// `AlbumsUi` field-for-field.
pub struct PlaylistsUi {
    grid: PlaylistGridState,
    detail: PlaylistDetailState,
    /// Row-tier cache — shared with Tracks / Browse — backs the
    /// detail track-list's artwork column.
    cover_thumbs: Arc<CoverThumbs>,
    /// Grid-tier (`GRID_COVER_FALLBACK`) cache for the Playlists grid cards.
    /// Released when the user opens a detail (the grid is unmounted) and
    /// when leaving the section. Re-warmed on return.
    grid_covers: Arc<CoverThumbs>,
    /// Detail-tier `(cover, blur)` pair cache for the Playlist Detail
    /// header. Released on detail-close and on section-leave.
    detail_artwork: Arc<DetailArtwork>,
    /// Section-visibility + staleness bookkeeping (on-screen shadow, dirty
    /// flag, and the wipe/fetch mutation gate). See [`SectionState`].
    section: SectionState,
}

impl PlaylistsUi {
    fn new(cover_thumbs: Arc<CoverThumbs>, hero_blur: Option<BlurSpec>) -> Self {
        Self {
            grid: PlaylistGridState {
                data: Mutex::new(Arc::new(GridData::new(Vec::new()))),
                index_cache: Mutex::new(None),
            },
            detail: PlaylistDetailState {
                tracks: Mutex::new(Vec::new()),
                all_tracks: Mutex::new(Vec::new()),
                position_order: Mutex::new(Vec::new()),
                playlist_id: Mutex::new(-1),
                applied_selection: Mutex::new(HashSet::new()),
                filter: Mutex::new(Needle::default()),
            },
            cover_thumbs,
            grid_covers: Arc::new(CoverThumbs::with_config(
                GRID_COVER_FALLBACK,
                DEFAULT_GRID_COVER_CAP,
            )),
            detail_artwork: Arc::new(DetailArtwork::new(hero_blur)),
            section: SectionState::new(),
        }
    }

    /// Drop everything resident in the Playlists section — grid covers,
    /// detail artwork pair, canonical grid data, memoized indices,
    /// cached detail tracks, applied-selection shadow — then return the
    /// freed pages to the OS. Race-correct against an in-flight
    /// `fetch_grid` via `mutation_gate` (see `AlbumsUi::release_section_state`
    /// for the contract).
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
            self.detail.position_order.lock().clear();
            self.detail.applied_selection.lock().clear();
        }
        crate::services::platform::allocator::trim();
    }

    /// Drop just the grid-tier cover cache — called when the user opens
    /// a playlist (the grid is unmounted).
    pub fn release_grid_covers(&self) {
        self.grid_covers.clear();
        crate::services::platform::allocator::trim();
    }

    /// Drop just the detail-tier `(cover, blur)` pair cache — called
    /// when the user closes a playlist detail.
    pub fn release_detail_artwork(&self) {
        self.detail_artwork.clear();
        crate::services::platform::allocator::trim();
    }

    pub fn prewarm_visible_covers(&self) {
        let data = self.grid.data.lock().clone();
        let unique = grid::first_screenful_paths(&data);
        if !unique.is_empty() {
            self.grid_covers.prewarm(&unique);
        }
    }

    /// Playlist id currently open in the detail view (`-1` = grid).
    pub fn detail_playlist_id(&self) -> i64 {
        *self.detail.playlist_id.lock()
    }

    /// Whether the cached grid holds any smart playlist whose criteria depend on
    /// play stats. Gates the `stats_changed` subscriber so a play-count flush
    /// only refreshes when a smart playlist could actually have changed — a
    /// static-rule ("Genre is Rock") smart playlist is skipped entirely. Stricter
    /// than a plain "any smart playlist?" check, and the correct gate for the
    /// stats path (a static-rule smart playlist can't have moved on a play flush).
    pub fn has_stat_dependent_smart_playlists(&self) -> bool {
        self.grid.data.lock().has_stat_dependent()
    }

    /// Whether the playlist with `id` is a stat-dependent smart playlist. Gates
    /// the open-detail refresh on the `stats_changed` path — a play stat change
    /// can't move a static-rule smart playlist, so its detail need not re-evaluate.
    pub fn is_playlist_smart_stat_dependent(&self, id: i64) -> bool {
        self.grid
            .data
            .lock()
            .row_stats_by_id(id)
            .is_some_and(|(_, _, stat_dependent)| stat_dependent)
    }

    /// Lazy cover lookup for a Playlists **grid card** — backs
    /// `Playlists.request-cover`. Resolves against the grid-tier cache.
    pub fn grid_cover(&self, artwork_path: &str) -> slint::Image {
        self.grid_covers
            .get_or_schedule_opt(crate::ui::grid_prewarm::nonempty_artwork_path(artwork_path))
    }

    /// The grid tier itself, for the wiring that has to reach past a lookup — the
    /// `Albums::grid_thumbs` contract.
    pub fn grid_thumbs(&self) -> Arc<CoverThumbs> {
        self.grid_covers.clone()
    }

    /// The inline sibling, for the Edit Artwork dialog's current-cover slot: it is written once as
    /// a property rather than read from a binding, so a scheduled decode has nothing to re-run and
    /// the placeholder would stand for the life of the dialog.
    pub fn grid_cover_blocking(&self, artwork_path: &str) -> slint::Image {
        crate::ui::grid_prewarm::grid_cover_blocking(&self.grid_covers, artwork_path)
    }

    /// Lookup of a playlist's canonical stats by id, against the cached
    /// grid `PlaylistStats` list. Clones the hit so the call site doesn't
    /// hold a lock across UI work. Backs `Playlists.row-name` /
    /// `row-description` / `request-edit-artwork-for` — grid-overlay
    /// callbacks that only know the playlist id.
    pub fn grid_stats_by_id(&self, id: i64) -> Option<PlaylistStats> {
        let data = self.grid.data.lock().clone();
        data.playlists.iter().find(|p| p.id == id).cloned()
    }

    /// Row-tier cover lookup — backs
    /// `Playlists.request-row-cover`, used by the mosaic-picker
    /// dialog's 64 px candidate tiles and preview slots. Resolves
    /// against the shared `cover_thumbs` LRU (the same one the detail
    /// track-list artwork column uses), so the dialog doesn't pay for
    /// a full-size grid-tier decode for a 64 px tile.
    pub fn row_cover(&self, artwork_path: &str) -> slint::Image {
        self.cover_thumbs.get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }
}

/// Build the empty `VecModel`s the Playlists grid (chunked + flat),
/// the detail track list, and the detail selection need, and hand them
/// to the Slint globals as `ModelRc`s.
fn install_models(ui: &AppWindow) {
    let grid: Rc<VecModel<UiPlaylistGridRow>> = Rc::new(VecModel::default());
    ui.global::<Playlists>().set_grid_rows(ModelRc::from(grid));

    let flat: Rc<VecModel<UiPlaylistRow>> = Rc::new(VecModel::default());
    ui.global::<Playlists>().set_rows(ModelRc::from(flat));

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<PlaylistDetail>().set_tracks(ModelRc::from(tracks));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<PlaylistDetail>().set_selected_ids(ModelRc::from(sel));
}

/// Convert a `PlaylistStats` into the Slint `PlaylistRow` an `EntityCard`
/// (and the detail header) renders. Pure data — the cover is *not*
/// decoded here; it's pulled lazily per visible card via `request-cover`.
pub fn to_slint_playlist_row(p: &PlaylistStats) -> UiPlaylistRow {
    UiPlaylistRow {
        id: clamp_i64_to_i32(p.id),
        name: SharedString::from(p.name.as_str()),
        description: SharedString::from(p.description.as_deref().unwrap_or("")),
        artwork_path: SharedString::from(p.thumbnail_path.as_deref().unwrap_or("")),
        track_count: p.track_count,
        total_duration_ms: i32::try_from(p.total_duration_ms.clamp(0, i64::from(i32::MAX)))
            .unwrap_or(i32::MAX),
        is_smart: p.is_smart,
    }
}

impl_section_state_helpers!(PlaylistsUi);
impl_detail_row_cache!(PlaylistsUi);

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<PlaylistsUi>();
};

#[cfg(test)]
#[path = "tests/playlists_tests.rs"]
mod tests;
