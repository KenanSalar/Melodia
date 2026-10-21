//! Tracks-view glue between Rust and Slint.
//!
//! The Slint `Tracks` global owns a [`TrackRows`] set once at startup via
//! `install_tracks_model`. It holds the library's ids in the current sort and
//! filter, and only the rows near the viewport; a sort or a filter reloads the
//! ids from SQL (`crate::ui::paged_rows`).
//!
//! Cross-thread layout:
//! * `TracksUi` is `Send + Sync` — `Arc<TracksUi>` cloned into callback
//!   closures and tokio tasks.
//! * Slint properties / model can only be touched from the UI thread; a load
//!   reaches back through the `Tracks` global's own weak handle.

mod callbacks;
mod fetch;

use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::{ComponentHandle, Global as _, ModelRc, SharedString, VecModel, Weak};

use crate::ui::paged_rows::ScrollPoint;
use crate::ui::paged_rows::track_rows::{self, ListLoads, TrackRows};
use crate::ui::section_state::{SectionState, impl_section_state_helpers};
use crate::ui::util::{StringPool, clamp_i64_to_i32};
use crate::ui::view_ctx::ViewCtx;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_ui::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

// `boot::ui_setup` kicks the first fetch after the window is shown, which is
// why `refresh` can't fold into `install` with the rest.
pub use fetch::{SongsQuery, refresh};

// Reached only from this slice's own `callbacks`. `pub(super)` is
// `pub(in crate::ui)` here.
pub(super) use fetch::requery;

/// Install the Tracks models, build the handle, and wire every `Tracks.*`
/// callback to it.
///
/// The returned handle is read by `install_views` for the initial fetch, the
/// `library_changed` refresher and the now-playing row mirrors. It is not a
/// keepalive; see [`crate::ui::albums::install`].
pub fn install(cx: ViewCtx<'_>) -> Arc<TracksUi> {
    let scroll = install_tracks_model(cx.app, cx.state);
    install_selection_model(cx.app);
    let tracks_ui = Arc::new(TracksUi {
        cover_thumbs: cx.cover_thumbs.clone(),
        section: SectionState::new(),
        global: cx.app.global::<Tracks>().as_weak(),
        loads: ListLoads::new(scroll),
        query: Mutex::new(SongsQuery::new("", "", "")),
    });
    callbacks::wire(cx.app, cx.state, cx.view_state, &tracks_ui);
    tracks_ui
}

/// What the Songs tab's callbacks and loads share across threads.
pub struct TracksUi {
    /// The shared row tier, which a refresh prewarms for the rows it opens
    /// on. The now-playing bar reads the same tier, so its tile reuses what
    /// the Tracks view warmed.
    cover_thumbs: Arc<CoverThumbs>,
    /// Visibility and staleness bookkeeping (`section-active-changed`
    /// shadow plus dirty flag). Unlike the entity-grid views, Tracks
    /// releases nothing on leave — `dirty` is set by a launch that doesn't
    /// show the tab and by the `library_changed` refresher when a bump
    /// arrives while the section is hidden, and consumed on re-enter to run
    /// one deferred fetch.
    section: SectionState,
    global: Weak<Tracks<'static>>,
    loads: ListLoads,
    /// The sort and the filter the next load runs under, written where they are picked and read
    /// once the load holds its ticket.
    query: Mutex<SongsQuery>,
}

impl_section_state_helpers!(TracksUi);

/// Hand the Slint `Tracks` global an empty [`TrackRows`], installed once:
/// every later keyset swaps inside it, `Repeater` comparing models by pointer.
fn install_tracks_model(ui: &AppWindow, state: &AppState) -> ScrollPoint {
    let model: Rc<TrackRows> = TrackRows::new(track_rows::fetcher(state));
    let scroll = model.scroll_point();
    ui.global::<Tracks>().set_rows(ModelRc::from(model));
    scroll
}

/// Install a persistent `VecModel<i32>` for `Tracks.selected-ids` so
/// selection mutations can `set_vec` into the same model instead of
/// allocating a fresh `ModelRc + VecModel` on every click.
fn install_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Tracks>().set_selected_ids(ModelRc::from(model));
}

/// Build a track-list row for the Slint model, every text field but the title, which rarely
/// repeats, drawn from `pool`.
///
/// Every field is `Send`, covers included — a row carries only its artwork *path* and
/// `RowCovers.request` resolves the thumbnail per instantiated row on the Slint side — so this
/// runs wherever the rows are, worker or event loop. It replaced a `prepare`/`finish` pair that
/// split it across a thread hop the finished row makes just as well.
pub fn to_slint_track_list_row(r: &RsTrackListRow, pool: &mut StringPool) -> UiTrackListRow {
    UiTrackListRow {
        id: clamp_i64_to_i32(r.id),
        title: SharedString::from(r.title.as_str()),
        artist: pool.intern(r.artist.as_deref().unwrap_or_default()),
        album: pool.intern(r.album.as_deref().unwrap_or_default()),
        genre: pool.intern(r.genre.as_deref().unwrap_or_default()),
        year: r.year.unwrap_or(0),
        track_number: r.track_number.unwrap_or(0),
        duration_ms: i32::try_from(r.duration_ms.clamp(0, i64::from(i32::MAX))).unwrap_or(i32::MAX),
        is_favorite: r.is_favorite,
        rating: r.rating,
        artwork_path: pool.intern(r.artwork_path.as_deref().unwrap_or_default()),
        display_duration: pool.intern(&format_duration_ms(r.duration_ms.max(0))),
        selected: false,
        // Always interactive for real DB-backed rows. Only the Browse view
        // overrides this `false` — for disk-only files not yet in the library.
        enabled: true,
        album_id: r.album_id.map_or(0, clamp_i64_to_i32),
        artist_id: r.artist_id.map_or(0, clamp_i64_to_i32),
        genre_id: r.genre_id.map_or(0, clamp_i64_to_i32),
    }
}

/// [`to_slint_track_list_row`] over a whole list, sharing one pool across it.
pub fn to_slint_track_list_rows<'a>(
    rows: impl IntoIterator<Item = &'a RsTrackListRow>,
) -> Vec<UiTrackListRow> {
    let mut pool = StringPool::default();
    rows.into_iter().map(|r| to_slint_track_list_row(r, &mut pool)).collect()
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
    if hours > 0 { format!("{hours}:{mins:02}:{secs:02}") } else { format!("{mins}:{secs:02}") }
}
