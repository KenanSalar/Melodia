//! Playlists-view glue between Rust and Slint.
//!
//! Two Slint globals are driven from here:
//!
//! * `Playlists` — the responsive playlist-card grid, a paged grid of the
//!   playlists' ids in the current sort and filter, its loads `ui::card_grid`'s.
//!   Per-card cover lookup is lazy via `request-cover`. `Playlists.count`, which
//!   every row menu's Add to Playlist entry is gated on, is kept current from
//!   launch by every playlist write, whatever tab is up.
//!
//! * `PlaylistDetail` — the full playlist detail view. `open_playlist`
//!   fetches the header and the playlist's tracks as a keyset in a
//!   [`TrackRows`], whose [`DetailCache`] holds the sort and filter its loads
//!   run under. In its own order the keyset *is* the playlist's, so a drag
//!   moves one id in it.
//!
//! Cross-thread layout mirrors `albums.rs`: `PlaylistsUi` is `Send + Sync`,
//! cloned into callbacks / tokio tasks; Slint properties and models are
//! only touched from the UI thread via
//! `Weak<AppWindow>::upgrade_in_event_loop`.

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

// Reached from outside the slice: `ui::nav_history` replays a walk into a
// detail, `boot::ui_setup` seeds the persisted one, and its
// `initial_grid_fetch!` kicks the first load after the window is shown
// — which is why `reload_grid` can't fold into `install` with the rest.
pub use detail::{open_playlist_with, seed_detail_from_settings};
pub use grid::reload_grid;

// Reached from this slice's own `callbacks/`. `pub(super)` is `pub(in crate::ui)`
// here, one notch wider than that.
pub(super) use detail::{apply_optimistic_reorder, open_playlist, refresh_detail, restore_detail};
pub(super) use grid::{refresh_after_write, refresh_play_stats, shown_card};

/// The M3U8 import / export and Add-to-Playlist wiring, kept out of [`install`] because each
/// needs the `Rc<NotificationsUi>` for its completion toasts and that is created after the
/// per-view wiring runs. Boot's `install_ui` calls it once the stack exists.
pub use callbacks::wire_files;

/// Install the Playlists grid + detail models, build the handle, and wire every
/// `Playlists.*` / `PlaylistDetail.*` callback to it, bar the ones [`wire_files`] holds.
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

/// Rust-side state for the Playlists grid + detail views. Shared between
/// the UI callbacks (`callbacks::wire`) and the async loads. Mirrors
/// `AlbumsUi` field-for-field.
pub struct PlaylistsUi {
    grid: GridLoads,
    detail: DetailCache<PlaylistDetail<'static>>,
    /// Row-tier cache — shared with Tracks / Browse — backs the
    /// detail track-list's artwork column.
    /// The grid cards draw from `ui::grid_prewarm::tier()`, which no view holds.
    cover_thumbs: Arc<CoverThumbs>,
    /// Detail-tier `(cover, blur)` pair cache for the Playlist Detail
    /// header. Released on detail-close and on section-leave.
    detail_artwork: Arc<DetailArtwork>,
    /// Section-visibility + staleness bookkeeping (on-screen shadow, dirty
    /// flag, and the wipe/fetch mutation gate). See [`SectionState`].
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

    /// Drop just the detail-tier `(cover, blur)` pair cache — called
    /// when the user closes a playlist detail.
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

    /// Playlist id currently open in the detail view (`-1` = grid).
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

/// Build the empty models the Playlists grid, the detail track list, and the detail selection
/// need, and hand them to the Slint globals as `ModelRc`s. The grid is a `PagedGrid` and the track
/// list a [`TrackRows`], each installed once.
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

/// Convert a `PlaylistStats` into the Slint `PlaylistRow` an `EntityCard`
/// (and the detail header) renders. Pure data — the cover is *not*
/// decoded here; it's pulled lazily per visible card via `request-cover`.
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

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<PlaylistsUi>();
};

#[cfg(test)]
#[path = "tests/playlists_tests.rs"]
mod tests;
