//! Tracks-view glue between Rust and Slint.
//!
//! The Slint `Tracks` global owns a `Rc<VecModel<TrackListRow>>` set
//! once at startup via `install_tracks_model`. The canonical (unfiltered) list
//! lives in `TracksUi::cache` so client-side filter changes don't hit the DB.
//!
//! Cross-thread layout:
//! * `TracksUi` is `Send + Sync` — `Arc<TracksUi>` cloned into callback
//!   closures and tokio tasks.
//! * Slint properties / model can only be touched from the UI thread; we
//!   reach back via `Weak<AppWindow>::upgrade_in_event_loop`.
//!
//! Allocation strategy is [`crate::ui::track_list_cache`]'s and is argued
//! there: this is the largest list in the app, so it retains *converted*
//! rows and a rebuild clones them (refcounted `SharedString`s) rather than
//! rebuilding them from DB rows. A refilter takes one lock and one
//! `Arc::clone`, and allocates nothing per row.

mod callbacks;
mod fetch;
mod selection;

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::track_list_cache::TrackListCache;
use crate::ui::util::clamp_i64_to_i32;
use crate::ui::view_ctx::ViewCtx;
use crate::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

// `boot::ui_setup` kicks the first fetch after the window is shown, which is
// why `fetch_and_apply` can't fold into `install` with the rest.
pub use fetch::fetch_and_apply;

// Reached only from this slice's own `callbacks.rs`, which used to live two
// modules away, plus the cross-slice `apply_row_*` mirrors in
// `callbacks::now_playing`. `pub(super)` is `pub(in crate::ui)` here, which is
// exactly that reach.
pub(super) use fetch::{apply_row_favorite, apply_row_rating, refilter, resort_and_apply};
pub(super) use selection::{clear_selection, handle_select_row};

/// Install the Tracks models, build the handle, and wire every `Tracks.*`
/// callback to it.
///
/// The returned handle is read by `install_views` for the initial fetch and the
/// `library_changed` refresher. It is not a keepalive; see
/// [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<TracksUi> {
    install_tracks_model(cx.app);
    install_selection_model(cx.app);
    let tracks_ui = Arc::new(TracksUi::new(cx.cover_thumbs.clone()));
    callbacks::wire(cx.app, cx.state, &tracks_ui);
    tracks_ui
}

/// Holds the unfiltered set of tracks the Tracks view is currently sorted by.
/// Filter changes re-derive the visible model from this cache without a DB hit.
pub struct TracksUi {
    /// Canonical row set plus its filter keys, sort keys and display-order
    /// permutation. A header-click sort recomputes only the permutation.
    pub(super) cache: TrackListCache,
    /// Path-keyed thumbnail cache. Many tracks share an album cover, so
    /// hitting this from `to_slint_track_list_row` avoids re-decoding the
    /// same file per track. Shared with the now-playing-bar bridge so
    /// the bar's artwork tile reuses cached thumbnails warmed by the
    /// Tracks view.
    pub(super) cover_thumbs: Arc<CoverThumbs>,
    /// Visibility and staleness bookkeeping (`section-active-changed`
    /// shadow plus dirty flag). Unlike the entity-grid views, Tracks
    /// releases nothing on leave — `dirty` is set only by the
    /// `library_changed` refresher when a bump arrives while the section
    /// is hidden, and consumed on re-enter to run one deferred re-fetch.
    section: SectionState,
}

impl TracksUi {
    fn new(cover_thumbs: Arc<CoverThumbs>) -> Self {
        Self {
            cache: TrackListCache::new(),
            cover_thumbs,
            section: SectionState::new(),
        }
    }

    /// IDs of the rows that pass `filter`, in the current sort order.
    /// `play-row` hands these to `player_play_tracks`, so the queue becomes
    /// exactly the view the user was looking at.
    pub fn current_ids_filtered(&self, filter: &str) -> Vec<i64> {
        // One lock, one `Arc::clone`, then walk off it — a concurrent fetch
        // can swap the cache underneath but not tear the set we hold.
        self.cache.snapshot().ids_filtered(&crate::ui::row_match::fold_needle(filter))
    }

    /// Surgical mutation of `is_favorite` on the cached row. Combined with
    /// `apply_row_favorite` below, lets us avoid re-fetching the whole list
    /// on a single-row favourite toggle (preserves scroll position + no flash).
    pub fn flip_favorite(&self, id: i64, fav: bool) {
        self.cache.set_favorite(id, fav);
    }

    /// Surgical mutation of `rating` on the cached row — the star-rating
    /// analogue of [`Self::flip_favorite`]. Paired with `apply_row_rating` so a
    /// hover-set rating doesn't re-fetch the whole list.
    pub fn flip_rating(&self, id: i64, rating: i32) {
        self.cache.set_rating(id, rating);
    }
}

impl_section_state_helpers!(TracksUi);

/// Build an empty `VecModel<TrackListRow>`, hand it to the Slint `Tracks`
/// global as a `ModelRc`. Subsequent updates locate it by downcasting
/// `Tracks.rows` back to `VecModel<TrackListRow>`.
fn install_tracks_model(ui: &AppWindow) {
    let model: Rc<VecModel<UiTrackListRow>> = Rc::new(VecModel::default());
    ui.global::<Tracks>().set_rows(ModelRc::from(model));
}

/// Install a persistent `VecModel<i32>` for `Tracks.selected-ids` so
/// selection mutations can `set_vec` into the same model instead of
/// allocating a fresh `ModelRc + VecModel` on every click.
fn install_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Tracks>().set_selected_ids(ModelRc::from(model));
}

/// Build a track-list row for the Slint model.
///
/// Every field is `Send`, covers included — a row carries only its artwork *path* and
/// `RowCovers.request` resolves the thumbnail per instantiated row on the Slint side — so this
/// runs wherever the rows are, worker or event loop. It replaced a `prepare`/`finish` pair that
/// split it across a thread hop the finished row makes just as well.
pub fn to_slint_track_list_row(r: &RsTrackListRow) -> UiTrackListRow {
    UiTrackListRow {
        id: clamp_i64_to_i32(r.id),
        title: SharedString::from(r.title.as_str()),
        artist: SharedString::from(r.artist.as_deref().unwrap_or("")),
        album: SharedString::from(r.album.as_deref().unwrap_or("")),
        genre: SharedString::from(r.genre.as_deref().unwrap_or("")),
        year: r.year.unwrap_or(0),
        track_number: r.track_number.unwrap_or(0),
        duration_ms: i32::try_from(r.duration_ms.clamp(0, i64::from(i32::MAX))).unwrap_or(i32::MAX),
        is_favorite: r.is_favorite,
        rating: r.rating,
        artwork_path: SharedString::from(r.artwork_path.as_deref().unwrap_or("")),
        display_duration: SharedString::from(format_duration_ms(r.duration_ms.max(0))),
        selected: false,
        // Always interactive for real DB-backed rows. Only the Browse view
        // overrides this `false` — for disk-only files not yet in the library.
        enabled: true,
        album_id: r.album_id.map_or(0, clamp_i64_to_i32),
        artist_id: r.artist_id.map_or(0, clamp_i64_to_i32),
        genre_id: r.genre_id.map_or(0, clamp_i64_to_i32),
    }
}

/// `mm:ss` for tracks under one hour, `h:mm:ss` otherwise. Matches the Slint
/// `Theme.format-duration` helper exactly so any UI surface that still calls
/// the Slint version (the now-playing bar's drag tooltip + total length) and
/// the precomputed row strings stay byte-identical.
pub(crate) fn format_duration_ms(ms: i64) -> String {
    let secs_total = ms / 1000;
    let hours = secs_total / 3600;
    let mins = (secs_total / 60) % 60;
    let secs = secs_total % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}
