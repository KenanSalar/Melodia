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

/// An album's header and the genre its hero names, if most of its tracks share one. Shared by
/// [`open_album`] (fresh user open) and [`refresh_detail`] (watcher-driven refresh).
async fn fetch_album_header(
    state: &AppState,
    album_id: i64,
) -> AppResult<(AlbumStats, Option<String>)> {
    let detail = library::albums::get_album_detail(&state.db, album_id).await?;
    let lines = library::keyset::genre_lines(&state.db, TrackListScope::Album(album_id)).await?;
    Ok((detail, dominant_genre(&lines)))
}

/// Fetch an album's header and track list and populate the `AlbumDetail` global,
/// whose `album-id >= 0` swaps the grid for the detail view. Fresh-open
/// semantics: resets the sort to the track-number default and clears any
/// selection, where the watcher-driven [`refresh_detail`] preserves both.
pub async fn open_album(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_album_with(state, albums_ui, weak, album_id, enter_from, |_ui| {}).await
}

/// [`open_album`] with a hook into the **same** `upgrade_in_event_loop` closure
/// that writes `album-id`, running after every detail property is set — so a
/// follow-on global write (`Nav.selected-index` for a cross-tab drill) lands in
/// the same frame and Slint paints `AlbumDetailBody` with no grid frame in
/// between. What follows the hook is the handful of writes that must read the
/// tab it may have just moved: the shared-hero gate, the history record and the
/// filter-box reseat.
///
/// `enter_from` is the enter direction of the **page** mount a cross-section
/// drill produces; `AlbumDetailBody` takes a fixed `above` and holds still while
/// the band morphs, so it reaches nothing when `Nav.selected-index` doesn't move
/// in the same tick.
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

    // One shared sort for every album, restored across opens and restarts.
    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::ALBUM_DETAIL, "track_number");
    let tracks = load::open(state, &**albums_ui, album_id, &sort_field, &sort_dir).await?;

    // Both halves come off one source decode. The buffers stay raw RGB8 so they
    // can cross the `upgrade_in_event_loop` boundary below.
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
        // The sort, an empty needle, no selection and the album's rows. The page's one filter box
        // is reseated at the end of this closure.
        tracks.seat(&state, &albums_ui);
        // Marked before the hook can flip `Nav.selected-index`, so a
        // cross-section drill's new page samples it on first paint. Inert on a
        // same-page drill, whose body reads a fixed `below`.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_album_id(clamp_i64_to_i32(album_id));
        // After `album-id`, so whatever globals the hook writes land in the same
        // UI-thread tick as the detail flip.
        on_applied(&ui);
        // The two globals six heroes share, written last because their gate is
        // the **live** tab rather than the `section_active` shadow the
        // `SectionActiveGate` only updates next frame. Read before the hook, a
        // cross-section drill answers for the tab the user *left*.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Albums);
        // Off `detail` rather than the `AlbumRow` above, which carries neither
        // `disc_count` nor `is_compilation`.
        crate::ui::hero_chips::publish_album(&ui, &detail, genre.as_deref(), on_screen);
        // The blur cross-fades; the cover slot is written directly, the artwork
        // tile being covered by the next album's in one frame.
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        // After the hook, so a cross-tab drill records the post-flip section.
        // No-op while a Mouse-4/5 replay is in flight.
        crate::ui::nav_history::record_current(&ui);
        // The filter clear above is only half a clear: the page's one box is
        // `MyLibrary.filter`, and the sheet's `album-id` mirror can't announce a
        // re-open writing the *same* id — which a section re-enter over an open
        // detail is, leaving the box holding a needle this call just cleared.
        // Last in the closure because `sync_box` reads the mounted tab, which
        // `on_applied` may have just moved.
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-read an open album after a library change, **preserving** the sort, the filter and the
/// selection: the library-changed subscriber fires on every watcher and scan tick, so it must not
/// silently reset any of them. The list reloads its ids and re-reads the rows it holds, repainting
/// only those whose content moved. The header half bails if the detail was closed or navigated
/// away while the fetch was in flight.
pub async fn refresh_detail(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
) -> AppResult<()> {
    track_rows::reload(state, albums_ui).await?;
    let (detail, genre) = fetch_album_header(state, album_id).await?;

    // Re-decode the `(cover, blur)` pair — the artwork may have changed
    // (cover swap, replace-on-disk). `get_or_decode` is a cache hit when
    // the path is unchanged, so this is cheap in the steady state.
    let pair =
        decode_detail_pair(state, albums_ui.detail_artwork.clone(), detail.artwork_path.clone())
            .await;

    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<AlbumDetail>();
        // The detail view was closed (or switched to another album) while
        // the fetch was in flight — drop this stale refresh.
        if i64::from(g.get_album_id()) != album_id {
            return;
        }

        // Header is one row — always refresh it (artwork / counts may
        // have changed).
        g.set_album(to_slint_album_row(&detail));
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Albums);
        crate::ui::hero_chips::publish_album(&ui, &detail, genre.as_deref(), on_screen);
        // No fade on the refresh path — this is the same album, the
        // user did not navigate. Either it's a cache hit (no change) or
        // the cover/blur is being replaced in place.
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, on_screen);
    });
    Ok(())
}

/// Reopen the album that was visible at the last shutdown: now if the launch shows the Albums
/// tab, otherwise on that tab's first enter. Runs once at startup *after* [`super::install`], so
/// the `AlbumDetail` callbacks are live by the time `open_album`'s closure lands. A deleted album
/// just logs and leaves `album-id` at `-1`, so the grid renders.
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
        // Above, not a drill-in slide: nobody navigated into this album, it is their last view.
        if let Err(e) = open_album(&s, &au, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("albums::restore_detail open_album({id}): {}", describe(&e));
        }
        // Lowered however it went, and behind `open_album`'s own hop so the id is already in: an
        // album gone since the last session owes the grid back rather than an empty body.
        au.section.end_restore();
        // A grid handed back is warmed before the hop below shows it; a reopen turns this away.
        crate::ui::grid_prewarm::prewarm_off_thread(&au, AlbumsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<AlbumDetail>().set_restoring(false);
        });
    });
}
