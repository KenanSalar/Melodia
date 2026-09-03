//! The Search page: a query-driven mixed-results view over one
//! `library::search::search_all` per debounced keystroke.
//!
//! **Top Result** is a single featured card ranked by
//! `top_result::compute_top_result`. A genre can win it but has no strip of its
//! own — it is a route to a page, not a row of things to browse. **Songs** is a
//! `TrackList` showing a compact prefix by default; "Show all" swaps to the
//! whole backend result off the cached `last_results`, with no DB round-trip.
//! The **Albums and Artists strips** match through their own tracks as well as
//! by name, so a query reaching only track metadata still fills them rather than
//! leaving the page a lone Songs list with no Top Result. **Recent searches**
//! show only on an empty box, and a committed query joins them only after a
//! pause, so a brief hesitation mid-typing doesn't pollute the list.
//!
//! Cache discipline is `src/ui/favorites`': per-strip `CoverThumbs` LRUs
//! released on section leave. There is deliberately **no**
//! `library_changed` subscriber — Search is query-driven, and a scan
//! completing mid-query must not swap results out from under the user.

mod apply;
mod callbacks;
mod fetch;
mod selection;
mod state;
mod top_result;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AppWindow, EntityStripRow as UiEntityStripRow, Search, TrackListRow as UiTrackListRow,
};

use state::{ALBUM_STRIP_THUMB_SIZE, ARTIST_STRIP_THUMB_SIZE, STRIP_THUMB_CAP, SearchUiState};

pub(super) use selection::{clear_selection, handle_select_row, restamp_rows};

/// Install the Search models, build the handle, and wire every `Search.*`
/// callback to it.
///
/// The two peers are the cross-tab hand-offs, taken as parameters so the
/// ordering is a compile error rather than a comment. There is no initial
/// fetch — the page paints empty until the user types.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(
    cx: ViewCtx<'_>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
) -> Arc<SearchUi> {
    install_models(cx.app);
    let search_ui = Arc::new(SearchUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, cx.view_state, &search_ui, albums_ui, artists_ui);
    search_ui
}

/// Rust-side state for the Search view, shared between the UI callbacks and the
/// async fetchers behind an `Arc`.
pub struct SearchUi {
    inner: SearchUiState,
    /// The shared row tier, for the Songs `TrackList` column.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The two strip tiers, each also serving Top Result when it is of that
    /// kind — so clicking a strip card after the top card hits a warm LRU.
    pub(super) album_strip_thumbs: Arc<CoverThumbs>,
    pub(super) artist_strip_thumbs: Arc<CoverThumbs>,
    /// Section-visible shadow. `release_section_state` bails on it, so a quickly
    /// re-entered section doesn't have its caches cleared mid-refresh.
    section_active: AtomicBool,
    /// Bumped by every `commit-search`; the in-flight `kick_search` re-reads it
    /// once `search_all` resolves and drops its UI write if a newer keystroke
    /// superseded it. `BrowseUi::fetch_token`'s shape.
    pub(super) fetch_token: AtomicU64,
    /// Bumped by every keystroke; the delayed `schedule_history_add` bails if it
    /// moved, a newer keystroke meaning the user is still typing.
    pub(super) history_token: AtomicU64,
}

impl SearchUi {
    fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            inner: SearchUiState::new(),
            cover_thumbs,
            album_strip_thumbs: Arc::new(CoverThumbs::with_config(
                ALBUM_STRIP_THUMB_SIZE,
                STRIP_THUMB_CAP,
            )),
            artist_strip_thumbs: Arc::new(CoverThumbs::with_config(
                ARTIST_STRIP_THUMB_SIZE,
                STRIP_THUMB_CAP,
            )),
            section_active: AtomicBool::new(false),
            fetch_token: AtomicU64::new(0),
            history_token: AtomicU64::new(0),
        }
    }

    pub fn set_section_active(&self, active: bool) {
        self.section_active.store(active, Ordering::Relaxed);
    }

    pub fn section_active(&self) -> bool {
        self.section_active.load(Ordering::Relaxed)
    }

    /// Drop every section-local buffer and reset the cached results, so a hidden
    /// page holds nothing. Runs off the UI thread on section leave, bailing when
    /// the section is already active again — a user who flipped tabs and back
    /// before this ran. Release order is
    /// `FavoritesUi::release_section_state`'s.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.album_strip_thumbs.clear();
        self.artist_strip_thumbs.clear();
        *self.inner.last_results.lock() = None;
        self.inner.last_query.lock().clear();
        self.inner.applied_selection.lock().clear();
        crate::services::allocator::trim();
    }

    /// [`Self::release_section_state`]'s release, run while the user is still on
    /// Search — when the box goes empty. No `section_active()` bail: an explicit
    /// clear says "start fresh" rather than "mid-edit", and the empty state's
    /// Recent chips touch no strip LRU, so the freed pages stay freed until the
    /// next query commits.
    ///
    /// The trade is that clearing and retyping the same query re-decodes a
    /// strip's worth of covers. Worth it against keeping thumbnails warm with
    /// nothing on screen to display them.
    pub fn release_for_empty_query(&self) {
        self.album_strip_thumbs.clear();
        self.artist_strip_thumbs.clear();
        *self.inner.last_results.lock() = None;
        self.inner.last_query.lock().clear();
        self.inner.applied_selection.lock().clear();
        crate::services::allocator::trim();
    }

    pub(crate) fn state(&self) -> &SearchUiState {
        &self.inner
    }

    /// Backs `Search.request-album-strip-cover`.
    pub fn album_strip_cover(&self, artwork_path: &str) -> Image {
        self.album_strip_thumbs.get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Backs `Search.request-artist-strip-cover`.
    pub fn artist_strip_cover(&self, artwork_path: &str) -> Image {
        self.artist_strip_thumbs.get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }
}

/// Hand the global its five empty `VecModel`s. Later updates find them by
/// downcasting back, on the UI thread.
fn install_models(ui: &AppWindow) {
    let g = ui.global::<Search>();

    let tracks: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    g.set_tracks(ModelRc::from(tracks));

    let albums: Rc<VecModel<UiEntityStripRow>> = Rc::new(VecModel::default());
    g.set_album_rows(ModelRc::from(albums));

    let artists: Rc<VecModel<UiEntityStripRow>> = Rc::new(VecModel::default());
    g.set_artist_rows(ModelRc::from(artists));

    let recent: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
    g.set_recent_rows(ModelRc::from(recent));

    let sel: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    g.set_selected_ids(ModelRc::from(sel));
}

/// An `AlbumStats` as its strip row, subtitled with the artist name for parity
/// with the Most Played cards. These cards surface no play count.
pub fn to_slint_album_strip_row(a: &AlbumStats) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle: SharedString::from(a.artist_name.as_str()),
        artwork_path: SharedString::from(a.artwork_path.as_deref().unwrap_or("")),
        play_count: 0,
    }
}

/// An `ArtistStats` as its strip row, subtitled with the album count for parity
/// with the Favorite Artists grid. Localizing that label is the caller's — the
/// fetch runs on a worker with no locale context, so the apply path translates.
pub fn to_slint_artist_strip_row(a: &ArtistStats, subtitle: &str) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle: SharedString::from(subtitle),
        artwork_path: SharedString::from(a.image_path.as_deref().unwrap_or("")),
        play_count: 0,
    }
}

// `const _` is type-checked but never dead-code-flagged, so no `#[allow]` is owed.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<SearchUi>();
};
