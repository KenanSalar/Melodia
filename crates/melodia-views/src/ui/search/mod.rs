//! The Search page: one `library::search::search_all` per debounced keystroke, painted as a Top
//! Result card, a compact Songs list that "Show all" expands off the cached results, and album and
//! artist strips. Recent searches show on an empty box, a query joining them only after a pause.
//!
//! There is deliberately no `library_changed` subscriber: a scan landing mid-query must not swap
//! the results out from under the user. A row edit is patched instead, into the cached row
//! ([`SearchUi::flip_favorite`]) and the one on screen (`apply::apply_row_*`).

mod apply;
mod callbacks;
mod fetch;
mod state;
mod top_result;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::ui::albums::AlbumsUi;
use crate::ui::artists::ArtistsUi;
use crate::ui::util::{clamp_i64_to_i32, opt_shared};
use crate::ui::view_ctx::ViewCtx;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::artist::ArtistStats;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_ui::{
    AppWindow, EntityStripRow as UiEntityStripRow, Search, TrackListRow as UiTrackListRow,
};

use state::{ALBUM_STRIP_THUMB_SIZE, ARTIST_STRIP_THUMB_SIZE, STRIP_THUMB_CAP, SearchUiState};

pub(super) use apply::{apply_row_favorite, apply_row_rating};
pub(crate) use callbacks::RecentClearUndo;

/// Wires every `Search.*` callback. The two peers are the open-album and open-artist hand-offs,
/// parameters so the install order is a compile error. There is no initial fetch, the page
/// painting empty until the user types.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
pub fn install(
    cx: ViewCtx<'_>,
    albums_ui: &Arc<AlbumsUi>,
    artists_ui: &Arc<ArtistsUi>,
) -> Arc<SearchUi> {
    install_models(cx.app);
    let search_ui = Arc::new(SearchUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx, &search_ui, albums_ui, artists_ui);
    search_ui
}

/// The Search page's handle, shared by its callbacks and its off-thread fetches.
pub struct SearchUi {
    inner: SearchUiState,
    /// The shared row tier, for the Songs `TrackList` column.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// The two strip tiers, each also serving Top Result of its kind, so a strip card clicked
    /// after the top card is warm.
    pub(super) album_strip_thumbs: Arc<CoverThumbs>,
    pub(super) artist_strip_thumbs: Arc<CoverThumbs>,
    section_active: AtomicBool,
    /// Bumped per commit; a `kick_search` whose token moved drops its write.
    pub(super) fetch_token: AtomicU64,
    /// Bumped per keystroke; a pending `schedule_history_add` bails if it moved.
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

    /// Drops every section-local buffer and the cached results, off the UI thread, unless the
    /// user has already come back.
    pub fn release_section_state(&self) {
        if self.section_active() {
            return;
        }
        self.album_strip_thumbs.clear();
        self.artist_strip_thumbs.clear();
        *self.inner.last_results.lock() = None;
        self.inner.last_query.lock().clear();
        melodia_platform::services::platform::allocator::trim();
    }

    /// [`Self::release_section_state`]'s release without the bail, for a box emptied while on
    /// Search: the empty state's Recent chips touch no strip tier, so what is freed stays freed.
    /// Retyping the same query re-decodes a strip, the price of holding nothing behind the box.
    pub fn release_for_empty_query(&self) {
        self.album_strip_thumbs.clear();
        self.artist_strip_thumbs.clear();
        *self.inner.last_results.lock() = None;
        self.inner.last_query.lock().clear();
        melodia_platform::services::platform::allocator::trim();
    }

    /// Patches the cached row, which "Show all" and a re-sort re-derive the visible slice from.
    pub fn flip_favorite(&self, id: i64, fav: bool) {
        self.patch_cached_track(id, |t| t.is_favorite = fav);
    }

    /// [`Self::flip_favorite`]'s star-rating twin.
    pub fn flip_rating(&self, id: i64, rating: i32) {
        self.patch_cached_track(id, |t| t.rating = rating);
    }

    /// A no-op when the id isn't cached, the common case for an edit made from Now Playing.
    fn patch_cached_track(&self, id: i64, edit: impl FnOnce(&mut RsTrackListRow)) {
        if let Some(results) = self.inner.last_results.lock().as_mut()
            && let Some(track) = results.tracks.iter_mut().find(|t| t.id == id)
        {
            edit(track);
        }
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

/// Hands the global its empty `VecModel`s, which every later write downcasts back to.
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

/// An `AlbumStats` as its strip card, subtitled with the artist.
pub fn to_slint_album_strip_row(a: &AlbumStats) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle: SharedString::from(a.artist_name.as_str()),
        artwork_path: opt_shared(a.artwork_path.as_deref()),
        play_count: 0,
        is_favorite: false,
    }
}

/// An `ArtistStats` as its strip card. `subtitle` is the translated album count, which only the
/// UI thread can resolve.
pub fn to_slint_artist_strip_row(a: &ArtistStats, subtitle: &str) -> UiEntityStripRow {
    UiEntityStripRow {
        id: clamp_i64_to_i32(a.id),
        title: SharedString::from(a.name.as_str()),
        subtitle: SharedString::from(subtitle),
        artwork_path: opt_shared(a.image_path.as_deref()),
        play_count: 0,
        is_favorite: false,
    }
}

const _: fn() = || {
    fn check<T: Send + Sync>() {}
    check::<SearchUi>();
};

#[cfg(test)]
#[path = "tests/search_ui_tests.rs"]
mod tests;
