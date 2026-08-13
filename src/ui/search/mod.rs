//! Search-view glue between Rust and Slint.
//!
//! Drives the `Search` Slint global (sidebar index 0) — a query-driven
//! mixed-results page composed of:
//!
//! * Hero — centered `SearchBar` (autofocus). Each keystroke bumps a
//!   debounce tick; a 300 ms Timer in `melodia-ui/ui/views/search-view.slint` then
//!   fires `commit-search`, which calls `fetch::kick_search` to run
//!   the FTS5 + LIKE query via `library::search::search_all` on the
//!   read pool.
//! * Top Result — single featured card (album, artist OR genre) computed
//!   via `top_result::compute_top_result`'s 9-step ranking: all three exact
//!   matches, then all three starts-with, then first-of-each. A genre
//!   gets the card rather than a strip of its own — it is a route to a
//!   page, not a row of things to browse.
//! * Songs — shared `TrackList` bound to `Search.tracks`. Compact by
//!   default (`state::COMPACT_TRACK_LIMIT` rows); the "Show all N"
//!   toggle swaps to the whole backend result — capped by `search_all`'s
//!   own `LIMIT` — from the cached `last_results`, no DB round-trip.
//! * Albums + Artists strips — `EntityCard` in a horizontal scroller
//!   (max 20 each, the backend's cap). Both match through their own
//!   tracks as well as by name, so a query that only reaches track
//!   metadata — a song title, a year, a composer, a genre — fills them
//!   rather than leaving the page a lone Songs list with no Top Result.
//! * Recent searches — shown only when the `SearchBar` is empty;
//!   chip-style buttons backed by [`crate::services::search_history::
//!   SearchHistoryState`] (cap 10, JSON-persisted under
//!   `paths.search_history_path`). 2-second delay after the last
//!   keystroke before the committed query is added to history (so a
//!   brief mid-typing pause doesn't pollute the list).
//!
//! Cache discipline mirrors `src/ui/favorites`: per-strip `CoverThumbs`
//! LRUs released on Search-section leave so the hidden view's resident
//! footprint drops to ~0. Re-enter re-fetches via the next keystroke
//! (no `library_changed_tx` subscriber — Search is query-driven, not
//! data-driven; a scan completing mid-query MUST NOT swap results out
//! from under the user).

// `apply` / `fetch` / `top_result` were `pub` only so the callbacks tree could
// reach them from two modules away. That consumer is a descendant now.
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
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{
    AppWindow, EntityStripRow as UiEntityStripRow, Search, TrackListRow as UiTrackListRow,
};

use state::{
    ALBUM_STRIP_THUMB_SIZE, ARTIST_STRIP_THUMB_SIZE, STRIP_THUMB_CAP, SearchUiState,
};

pub(super) use selection::{clear_selection, handle_select_row, restamp_rows};

/// Install the Search models, build the handle, and wire every `Search.*`
/// callback to it.
///
/// Takes both peers because the cross-tab open-album / open-artist hand-offs
/// need live handles to call into — which is the ordering, made a compile error
/// rather than a comment: this does not resolve until both are bound. There is
/// no initial fetch; Search is query-driven, so the page paints empty until the
/// user types.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(
    cx: ViewCtx<'_>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
) -> Arc<SearchUi> {
    install_models(cx.app);
    let search_ui = Arc::new(SearchUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, &search_ui, albums_ui, artists_ui);
    search_ui
}

/// Rust-side state for the Search view. Shared between the UI
/// callbacks (`callbacks::wire`) and async fetchers behind an
/// `Arc<SearchUi>` — `Send + Sync`.
pub struct SearchUi {
    inner: SearchUiState,
    /// Shared row-tier cache — used for the Songs `TrackList`
    /// row column. Same instance every view consumes (cache parity
    /// with Tracks / Browse / Favorites is free).
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Albums-strip cache (180 px). Also serves Top Result when the
    /// top kind is `"album"` — the user clicking a strip card after
    /// the top card hits a warm LRU.
    pub(super) album_strip_thumbs: Arc<CoverThumbs>,
    /// Artists-strip cache (200 px, circular cards). Also serves Top
    /// Result when the top kind is `"artist"`.
    pub(super) artist_strip_thumbs: Arc<CoverThumbs>,
    /// Section-visible shadow — mirrors
    /// `Nav.selected-index == NAV_SEARCH && !Nav.now-playing-open`.
    /// Used by `release_section_state` as a race-safety bail-out so a
    /// quickly-reentered section doesn't have its caches cleared mid-
    /// refresh.
    section_active: AtomicBool,
    // (Note: spawn_blocking happens off the UI thread, not in this struct.)
    /// Monotonic fetch token. Every `commit-search` bumps it; the
    /// in-flight `kick_search` re-reads after `search_all` resolves
    /// and drops the UI write if a newer keystroke superseded it.
    /// Same shape as `BrowseUi::fetch_token`.
    pub(super) fetch_token: AtomicU64,
    /// Monotonic history-add token. Every keystroke bumps it; the
    /// 2-second-delayed `schedule_history_add` task bails if the
    /// token has moved (newer keystroke means the user is still
    /// typing, so don't write a stale query to history).
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

    /// Drop every section-local resident buffer + reset cached
    /// results so the hidden view's footprint drops to ~0. Called
    /// (off the UI thread) on section leave. Bails synchronously if
    /// the section is active again — covers a leave-then-quickly-re-
    /// enter race where the user flipped tabs and back before the
    /// `spawn_blocking` ran.
    ///
    /// Cache release ordering matches `FavoritesUi::release_section_state`:
    /// per-section LRUs first, then mutated state, then a
    /// `heap_trim::trim` so glibc hands the freed pages back.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.album_strip_thumbs.clear();
        self.artist_strip_thumbs.clear();
        *self.inner.last_results.lock() = None;
        self.inner.last_query.lock().clear();
        self.inner.applied_selection.lock().clear();
        // Hand freed pages back to glibc — same ordering as
        // FavoritesUi (per-section LRUs first, then state, then trim).
        crate::tasks::heap_trim::trim();
    }

    /// Same cache + state release as [`Self::release_section_state`] but
    /// runs while the user is still on Search — fired when the
    /// `SearchBar` text becomes empty. No `section_active()` bail-out:
    /// the user explicitly cleared the input, so we want to release
    /// immediately even though we're on the active tab. The empty-
    /// state branch (Recent chips) doesn't touch the strip LRUs, so
    /// the freed pages stay freed until the next query commits.
    ///
    /// Trade-off vs keeping the LRUs warm: a same-prefix follow-up
    /// query (`"metal"` → clear → `"metal"`) pays ~40 covers ×
    /// ~1-2 ms decode each on next commit. Worth it because (a) an
    /// explicit clear signals "start fresh", not "mid-edit", and
    /// (b) keeping ~12 MiB of strip thumbnails warm with nothing
    /// visible to display them on goes against the project's
    /// memory-discipline conventions.
    pub fn release_for_empty_query(&self) {
        self.album_strip_thumbs.clear();
        self.artist_strip_thumbs.clear();
        *self.inner.last_results.lock() = None;
        self.inner.last_query.lock().clear();
        self.inner.applied_selection.lock().clear();
        crate::tasks::heap_trim::trim();
    }

    pub(crate) fn state(&self) -> &SearchUiState {
        &self.inner
    }

    /// Lazy cover lookup for the Albums-strip cards. Routed via
    /// `Search.request-album-strip-cover`.
    pub fn album_strip_cover(&self, artwork_path: &str) -> Image {
        self.album_strip_thumbs
            .get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }

    /// Lazy cover lookup for the Artists-strip cards. Routed via
    /// `Search.request-artist-strip-cover`.
    pub fn artist_strip_cover(&self, artwork_path: &str) -> Image {
        self.artist_strip_thumbs
            .get_or_load_opt(Some(artwork_path).filter(|s| !s.is_empty()))
    }
}

/// Bind empty Slint `VecModel`s for the Songs list, the Albums + Artists
/// strips, the recent-searches list, and the selection set. Subsequent
/// updates locate them by downcasting back to `VecModel<T>` from the UI
/// thread.
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

/// Map an `AlbumStats` to its Slint strip row. Subtitle is the artist
/// name (parity with the Most Played cards). `play_count` is unused
/// (the cards don't surface it).
pub fn to_slint_album_strip_row(a: &AlbumStats) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle: SharedString::from(a.artist_name.as_str()),
        artwork_path: SharedString::from(a.artwork_path.as_deref().unwrap_or("")),
        play_count: 0,
    }
}

/// Map an `ArtistStats` to its Slint strip row. Subtitle is the
/// `{n} albums` count line (parity with Favorites' Favorite Artists
/// strip); the localised label is the caller's responsibility (the
/// fetch path lives on a tokio worker without locale context, so we
/// pass through whatever was supplied and let the apply path
/// translate). `play_count` is unused.
pub fn to_slint_artist_strip_row(a: &ArtistStats, subtitle: &str) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle: SharedString::from(subtitle),
        artwork_path: SharedString::from(a.image_path.as_deref().unwrap_or("")),
        play_count: 0,
    }
}

// Compile-time assertion, not runtime code: an anonymous `const _` is
// type-checked but never dead-code-flagged, so the bound is enforced
// without an `#[allow(dead_code)]` on a fn nothing calls.
const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<SearchUi>();
};
