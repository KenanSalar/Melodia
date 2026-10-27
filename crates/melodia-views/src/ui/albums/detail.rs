//! Album Detail's own half: the header and hero fetch, the artwork pair decode, open, refresh and
//! the startup seed. What the four details share is [`crate::ui::track_detail`].

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::{AlbumsUi, to_slint_album_row};
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_view::{apply_detail_artwork, resolve_view_sort};
use crate::ui::hero_folds::dominant_genre;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::paged_rows::track_rows;
use crate::ui::section_state::SectionState;
use crate::ui::track_detail::{DetailCache, TrackDetail, load, restore};
use crate::ui::track_list_view::view_id;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::album::AlbumStats;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AlbumDetail, AppWindow, NavEnterFrom};

impl TrackDetail for AlbumsUi {
    type Global = AlbumDetail<'static>;

    const TAB: MyLibraryTab = MyLibraryTab::Albums;

    fn cache(&self) -> &DetailCache<Self::Global> {
        &self.detail
    }

    fn section(&self) -> &SectionState {
        &self.section
    }

    fn row_covers(&self) -> &Arc<CoverThumbs> {
        &self.cover_thumbs
    }

    fn scope(&self, id: i64) -> TrackListScope {
        TrackListScope::Album(id)
    }
}

/// An album's header and the genre its hero names, if most of its tracks share one.
async fn fetch_album_header(
    state: &AppState,
    album_id: i64,
) -> AppResult<(AlbumStats, Option<String>)> {
    let detail = library::albums::get_album_detail(&state.db, album_id).await?;
    let lines = library::keyset::genre_lines(&state.db, TrackListScope::Album(album_id)).await?;
    Ok((detail, dominant_genre(&lines)))
}

/// Fetches an album's header and tracks into `AlbumDetail`, whose `album-id >= 0` swaps the grid
/// for the detail. A fresh open clears the filter and the selection, which [`refresh_detail`]
/// keeps.
pub async fn open_album(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_album_with(state, albums_ui, weak, album_id, enter_from, |_ui| {}).await
}

/// [`open_album`] with a hook run in the **same** closure that writes `album-id`, after every
/// detail property, so a follow-on write (`Nav.selected-index` for a cross-tab drill) lands in the
/// same frame and no grid frame paints between. What follows the hook is what must read the tab
/// it may have just moved: the shared-hero gate, the history record and the filter-box reseat.
///
/// `enter_from` is the enter direction of the **page** a cross-section drill mounts; the body
/// takes a fixed `above`, so it reaches nothing when the nav index stays put.
pub async fn open_album_with<F>(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
    enter_from: NavEnterFrom,
    on_applied: F,
) -> AppResult<()>
where
    F: FnOnce(&AppWindow) + Send + 'static,
{
    let (detail, genre) = fetch_album_header(state, album_id).await?;

    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::ALBUM_DETAIL, "track_number");
    let tracks = load::open(state, &**albums_ui, album_id, &sort_field, &sort_dir).await?;

    let pair =
        decode_detail_pair(state, albums_ui.detail_artwork.clone(), detail.artwork_path.clone())
            .await;

    albums_ui.detail.set_id(album_id);

    let state = state.clone();
    let albums_ui = albums_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<AlbumDetail>();
        let header = to_slint_album_row(&detail);
        g.set_album(header);
        tracks.seat(&state, &albums_ui);
        // Before the hook can move the nav index, so the page it mounts reads it on first paint.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_album_id(clamp_i64_to_i32(album_id));
        on_applied(&ui);
        // A live read after the hook: the `section_active` shadow only updates next frame, so a
        // cross-section drill would answer for the tab the user left.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Albums);
        // Off `detail`, the `AlbumRow` carrying neither `disc_count` nor `is_compilation`.
        crate::ui::hero_chips::publish_album(&ui, &detail, genre.as_deref(), on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        crate::ui::nav_history::record_current(&ui);
        // The seat cleared the detail's needle but not `MyLibrary.filter`, and the sheet's id
        // mirror can't see a re-open writing the same id. Last, because `sync_box` reads the tab
        // `on_applied` may have moved.
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-reads an open album after a library change, keeping the sort, the filter and the selection,
/// since the subscriber fires on every watcher and scan tick. The header half drops out if the
/// detail moved on while the fetch was in flight.
pub async fn refresh_detail(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
) -> AppResult<()> {
    track_rows::reload(state, albums_ui).await?;
    let (detail, genre) = fetch_album_header(state, album_id).await?;

    // The artwork may have been swapped; an unchanged path is a cache hit.
    let pair =
        decode_detail_pair(state, albums_ui.detail_artwork.clone(), detail.artwork_path.clone())
            .await;

    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<AlbumDetail>();
        if i64::from(g.get_album_id()) != album_id {
            return;
        }

        g.set_album(to_slint_album_row(&detail));
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Albums);
        crate::ui::hero_chips::publish_album(&ui, &detail, genre.as_deref(), on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, on_screen);
    });
    Ok(())
}

/// Reopens the album on screen at the last shutdown: now if the launch shows the Albums tab,
/// otherwise on that tab's first enter. Called after [`super::install`], so the callbacks are live
/// when `open_album` lands; an album since deleted leaves the grid up.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>) {
    restore::seed_or_defer(ui, state, albums_ui, restore_detail);
}

/// Reopen a persisted album, at launch or on the first enter of a tab the launch left hidden.
pub fn restore_detail(ui: &AppWindow, state: &AppState, albums_ui: &Arc<AlbumsUi>, id: i64) {
    // Synchronously, so it is up before the tab's first frame; see `AlbumDetail.restoring`.
    ui.global::<AlbumDetail>().set_restoring(true);
    albums_ui.section.begin_restore();
    let s = state.clone();
    let au = albums_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Above rather than a drill-in slide: nobody navigated here, it is their last view.
        if let Err(e) = open_album(&s, &au, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("albums::restore_detail open_album({id}): {}", describe(&e));
        }
        // However it went, and behind `open_album`'s hop so the id is in: an album gone since the
        // last session owes the grid back rather than an empty body.
        au.section.end_restore();
        // Warms a grid handed back before the hop below shows it; a reopen turns this away.
        crate::ui::grid_prewarm::prewarm_off_thread(&au, AlbumsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<AlbumDetail>().set_restoring(false);
        });
    });
}
