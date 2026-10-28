//! Playlist Detail's own half: the header and hero fetch, the artwork pair decode, open, refresh,
//! the startup seed, the drag that moves ids in the keyset, and whether a file dropped on the page
//! lands in it. What the four details share is [`crate::ui::track_detail`].

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::{PlaylistsUi, to_slint_playlist_row};
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_view::{apply_detail_artwork, resolve_view_sort};
use crate::ui::hero_folds::HeroFold;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::paged_rows::track_rows::{self, TrackRows};
use crate::ui::section_state::SectionState;
use crate::ui::track_detail::{DetailCache, TrackDetail, load, restore};
use crate::ui::track_list_view::view_id;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::playlist::PlaylistStats;
use melodia_core::entities::track_list::TrackListScope;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, DragOrigin, MiniPlayer, Nav, NavEnterFrom, PlaylistDetail};

/// The playlist's own order, which no column header asks for, so the sort cycle hands it back
/// (`next_sort_with_natural`).
pub const POSITION_FIELD: &str = "position";

/// What a miss on the page's row model is logged under.
const TRACKS_LABEL: &str = "PlaylistDetail.tracks";

impl TrackDetail for PlaylistsUi {
    type Global = PlaylistDetail<'static>;

    const NATURAL_SORT: Option<&'static str> = Some(POSITION_FIELD);
    const TAB: MyLibraryTab = MyLibraryTab::Playlists;

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
        TrackListScope::Playlist(id, self.draw())
    }

    /// A drag still in flight goes on, the list's source surviving the rows it holds, but the
    /// index it lifted no longer names the same track.
    fn rows_moving(&self) {
        if let Some(drag) = self.drag.upgrade() {
            drag.invoke_revoke(DragOrigin::Playlist);
        }
    }
}

/// The playlist a file let go of over the page lands in ([`DetailPage::drop_target`]).
pub fn detail_on_screen(ui: &AppWindow) -> Option<i64> {
    let detail = ui.global::<PlaylistDetail>();
    let page = DetailPage {
        covered: ui.global::<Nav>().get_now_playing_open()
            || ui.global::<MiniPlayer>().get_active(),
        tab_mounted: tab_is_mounted(ui, MyLibraryTab::Playlists),
        playlist_id: detail.get_playlist_id(),
        is_smart: detail.get_playlist().is_smart,
    };
    page.drop_target()
}

/// Playlist Detail as the window has it, which is all a file drop asks of the page.
pub(super) struct DetailPage {
    /// Now Playing or the miniplayer is drawn over it.
    pub(super) covered: bool,
    pub(super) tab_mounted: bool,
    pub(super) playlist_id: i32,
    pub(super) is_smart: bool,
}

impl DetailPage {
    /// The detail open on screen, and only one whose membership is its own, a smart playlist's
    /// following its rules.
    pub(super) fn drop_target(&self) -> Option<i64> {
        if self.covered || !self.tab_mounted {
            return None;
        }
        (self.playlist_id >= 0 && !self.is_smart).then_some(i64::from(self.playlist_id))
    }
}

/// Whether the rows on screen are in the playlist's own order, ascending: the whole precondition
/// for a drag's display index being a position. Descending shows the list reversed, and every
/// drag would move the mirrored row.
pub(super) fn is_manual_order(field: &str, dir: &str) -> bool {
    field == POSITION_FIELD && dir != "desc"
}

/// A playlist's header and how far its tracks spread. A smart playlist has no `playlist_items`
/// rows to keep its count and duration, so both come off the query its spread does, under the
/// draw its list was picked by, and its automatic cover off its rules, as on its card.
async fn fetch_playlist_header(
    state: &AppState,
    playlists_ui: &PlaylistsUi,
    playlist_id: i64,
) -> AppResult<(PlaylistStats, HeroFold)> {
    let mut detail = library::playlists::get_playlist_detail(&state.db, playlist_id).await?;
    let facts = library::keyset::facts(&state.db, playlists_ui.scope(playlist_id)).await?;
    if detail.is_smart {
        detail.track_count = clamp_i64_to_i32(facts.tracks);
        detail.total_duration_ms = facts.duration_ms;
    }
    library::smart_playlists::fill_automatic_thumbnail(&state.db, &mut detail).await;
    Ok((detail, HeroFold::from(&facts)))
}

/// Fetches a playlist's header and tracks into `PlaylistDetail`, as
/// [`crate::ui::albums::open_album_with`] does an album's, drawing a random-capped smart
/// playlist's pick afresh.
pub async fn open_playlist(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    playlist_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_playlist_with(state, playlists_ui, weak, playlist_id, enter_from, |_ui| {}).await
}

/// [`open_playlist`] with a hook, on [`crate::ui::albums::open_album_with`]'s contract.
pub async fn open_playlist_with<F>(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    playlist_id: i64,
    enter_from: NavEnterFrom,
    on_applied: F,
) -> AppResult<()>
where
    F: FnOnce(&AppWindow) + Send + 'static,
{
    playlists_ui.redraw();
    let (detail, fold) = fetch_playlist_header(state, playlists_ui, playlist_id).await?;

    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::PLAYLIST_DETAIL, POSITION_FIELD);
    let tracks = load::open(state, &**playlists_ui, playlist_id, &sort_field, &sort_dir).await?;

    let pair = decode_detail_pair(
        state,
        playlists_ui.detail_artwork.clone(),
        detail.thumbnail_path.clone(),
    )
    .await;

    // Before the UI hop, so a drag or a row play on the next tick already sees it.
    playlists_ui.detail.set_id(playlist_id);

    let state = state.clone();
    let playlists_ui = playlists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<PlaylistDetail>();
        let header = to_slint_playlist_row(&detail);
        g.set_playlist(header);
        tracks.seat(&state, &playlists_ui);
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_playlist_id(clamp_i64_to_i32(playlist_id));
        on_applied(&ui);
        // From here on, in `open_album_with`'s order and for its reasons.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Playlists);
        crate::ui::hero_chips::publish_playlist(&ui, &detail, fold, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        crate::ui::nav_history::record_current(&ui);
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-reads an open playlist after a library change or an edit as `albums::refresh_detail` does.
pub async fn refresh_detail(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    playlist_id: i64,
) -> AppResult<()> {
    track_rows::reload(state, playlists_ui).await?;
    refresh_header(state, playlists_ui, weak, playlist_id).await
}

/// Re-reads an open playlist's header and cover alone, for a write that left every row reading
/// the same: a move hands an automatic cover to whichever track now leads.
pub async fn refresh_header(
    state: &AppState,
    playlists_ui: &PlaylistsUi,
    weak: Weak<AppWindow>,
    playlist_id: i64,
) -> AppResult<()> {
    let (detail, fold) = fetch_playlist_header(state, playlists_ui, playlist_id).await?;

    let pair = decode_detail_pair(
        state,
        playlists_ui.detail_artwork.clone(),
        detail.thumbnail_path.clone(),
    )
    .await;

    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<PlaylistDetail>();
        if i64::from(g.get_playlist_id()) != playlist_id {
            return;
        }

        g.set_playlist(to_slint_playlist_row(&detail));
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Playlists);
        crate::ui::hero_chips::publish_playlist(&ui, &detail, fold, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, on_screen);
    });
    Ok(())
}

/// Moves the dragged rows together in the keyset ahead of the write, returning whether anything
/// moved. Whatever the write answers, the caller then reads the order back from the database.
pub fn apply_optimistic_move(
    ui: &AppWindow,
    playlists_ui: &PlaylistsUi,
    rows: &[usize],
    to: usize,
) -> bool {
    let g = ui.global::<PlaylistDetail>();
    let field = g.get_sort_field();
    let dir = g.get_sort_dir();
    // Asked again beside `reorder-enabled`, here being where display indices reach the keyset.
    // Filtered, the list is a subset and `to` lands among rows the playlist doesn't hold there,
    // the write still landing in range. The list on show takes a sort or needle a query after the
    // page, and an overtaken open can leave another playlist's rows up under this id.
    if !is_manual_order(&field, &dir)
        || playlists_ui.detail.is_filtered()
        || !playlists_ui.detail.shows_own_order()
    {
        return false;
    }
    let tracks = g.get_tracks();
    TrackRows::installed(&tracks, TRACKS_LABEL).is_some_and(|model| model.move_rows(rows, to))
}

/// Where the rows the page's picks name sit on show, in order.
pub fn picked_rows(ui: &AppWindow) -> Vec<usize> {
    let tracks = ui.global::<PlaylistDetail>().get_tracks();
    TrackRows::installed(&tracks, TRACKS_LABEL).map(TrackRows::picked_rows).unwrap_or_default()
}

/// The track ids on show at `rows`, read before a move renumbers them.
pub fn track_ids_at(ui: &AppWindow, rows: &[usize]) -> Vec<i64> {
    let ids = TrackRows::ids_of(&ui.global::<PlaylistDetail>().get_tracks(), TRACKS_LABEL);
    rows.iter().filter_map(|&row| ids.get(row).copied()).collect()
}

/// Reopens the playlist on screen at the last shutdown: now if the launch shows the Playlists
/// tab, otherwise on that tab's first enter.
pub fn seed_detail_from_settings(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
) {
    restore::seed_or_defer(ui, state, playlists_ui, restore_detail);
}

/// Reopen a persisted playlist, at launch or on the first enter of a tab the launch left hidden.
pub fn restore_detail(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>, id: i64) {
    // Synchronously, so it is up before the tab's first frame; see `PlaylistDetail.restoring`.
    ui.global::<PlaylistDetail>().set_restoring(true);
    playlists_ui.section.begin_restore();
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // In `albums::restore_detail`'s order and for its reasons.
        if let Err(e) = open_playlist(&s, &pu, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("playlists::restore_detail open_playlist({id}): {}", describe(&e));
        }
        pu.section.end_restore();
        crate::ui::grid_prewarm::prewarm_off_thread(&pu, PlaylistsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<PlaylistDetail>().set_restoring(false);
        });
    });
}
