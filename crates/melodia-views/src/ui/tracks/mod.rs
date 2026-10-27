//! The Songs tab: one [`TrackRows`], installed at startup, holding the library's ids in the
//! current sort and filter and only the rows near the viewport.

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

pub use fetch::{SongsQuery, refresh};

pub(super) use fetch::requery;

/// Installs the models and wires every `Tracks.*` callback.
///
/// The returned handle is not a keepalive; see [`crate::ui::albums::install`].
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
    /// The shared row tier, which a refresh prewarms for the rows it opens on, the now-playing
    /// bar's tile included.
    cover_thumbs: Arc<CoverThumbs>,
    /// Dirtied by a launch onto another tab and by a library change while hidden, and consumed
    /// by the re-enter's one deferred fetch; a leave releases nothing.
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

/// Installed once, so a selection change `set_vec`s into it rather than allocating a model per
/// click.
fn install_selection_model(ui: &AppWindow) {
    let model: Rc<VecModel<i32>> = Rc::new(VecModel::default());
    ui.global::<Tracks>().set_selected_ids(ModelRc::from(model));
}

/// A track-list row, every text field but the title, which rarely repeats, drawn from `pool`.
/// A row carries its artwork *path*, so every field is `Send` and this runs wherever the rows
/// are.
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
        // Browse alone sets `false`, for a file on disk the library doesn't hold.
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

/// `m:ss` under an hour, `h:mm:ss` from one: byte for byte `Theme.format-duration`, which the
/// Slint surfaces formatting live still call.
pub(crate) fn format_duration_ms(ms: i64) -> String {
    let secs_total = ms / 1000;
    let hours = secs_total / 3600;
    let mins = (secs_total / 60) % 60;
    let secs = secs_total % 60;
    if hours > 0 { format!("{hours}:{mins:02}:{secs:02}") } else { format!("{mins}:{secs:02}") }
}
