//! Playlist Detail's own half: the header and hero fetch, the artwork pair decode, open, refresh,
//! the startup seed, and the drag that moves one id in the keyset. What the four details share is
//! [`crate::ui::track_detail`].

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
use melodia_ui::{AppWindow, NavEnterFrom, PlaylistDetail};

/// The playlist's own curated order. Synthetic — no column header asks for it,
/// which is why the sort cycle has to hand it back (`next_sort_with_natural`).
pub const POSITION_FIELD: &str = "position";

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

    /// Abort any in-flight drag-reorder: the row indices it was computed against no longer
    /// describe the playlist, and the reset destroys the row instance holding the pointer grab, so
    /// it can never clear this state itself. Left set, the source row stays ghosted and the drop
    /// line stranded.
    fn rows_moving(&self, g: &Self::Global) {
        g.set_drag_source(-1);
        g.set_drop_slot(-1);
    }

    fn clear_detail(&self) {
        self.detail.clear();
        crate::ui::window_chrome::set_current_playlist_id(-1);
    }
}

/// Whether the rows on screen are in the playlist's own order, ascending.
///
/// The drag hands over *display* indices and [`apply_optimistic_reorder`] moves them in the
/// keyset, so this is the whole precondition for a display index being a position. The direction
/// half is the easy one to miss: a descending position sort shows the list reversed, which would
/// make every drag move the mirrored row.
pub(super) fn is_manual_order(field: &str, dir: &str) -> bool {
    field == POSITION_FIELD && dir != "desc"
}

/// A playlist's header and how far its tracks spread. A smart playlist has no `playlist_items`
/// rows, so its `track_count` and `total_duration_ms` stay 0 in the junction-maintained columns
/// and are counted off its current membership instead, the same query its spread comes from, under
/// the draw its list was picked by. Its automatic cover comes off its rules too, as on its card.
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

/// Fetch a playlist's header + track list and populate the `PlaylistDetail` global — which flips
/// `playlist-id >= 0`, swapping the grid for the detail view. Fresh-open semantics: restores the
/// persisted detail sort and clears any prior selection. The watcher-driven refresh uses
/// [`refresh_detail`] instead, which preserves both.
pub async fn open_playlist(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    playlist_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_playlist_with(state, playlists_ui, weak, playlist_id, enter_from, |_ui| {}).await
}

/// Same as [`open_playlist`] but the caller can hook into the **same** `upgrade_in_event_loop`
/// closure that writes `playlist-id`. The hook runs after every detail property is set, so a
/// follow-on global write — the tab and section flip a `nav_history` replay owes — lands in the
/// same frame, and Slint paints `PlaylistDetailBody` with no Playlists-grid frame in between.
///
/// `enter_from` chooses the enter direction for the **page** mount a cross-section arrival
/// produces; `PlaylistDetailBody` itself takes a fixed `above`, so it reaches nothing when
/// `Nav.selected-index` doesn't move in the same tick.
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

    // `position` is the fresh-install default; a persisted non-`position` sort is restored across
    // restarts.
    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::PLAYLIST_DETAIL, POSITION_FIELD);
    let tracks = load::open(state, &**playlists_ui, playlist_id, &sort_field, &sort_dir).await?;

    let pair = decode_detail_pair(
        state,
        playlists_ui.detail_artwork.clone(),
        detail.thumbnail_path.clone(),
    )
    .await;

    // Set before the UI hop so drag-reorder and play-row callbacks firing on the next tick
    // already see it.
    playlists_ui.detail.set_id(playlist_id);

    // Inform the OS file-drop coalescer that this playlist is the current drop target — used only
    // when the Queue Sheet is closed, the queue taking priority when both are open. A smart
    // playlist's membership is derived, so it registers none and drops fall through to import.
    crate::ui::window_chrome::set_current_playlist_id(if detail.is_smart {
        -1
    } else {
        playlist_id
    });

    let state = state.clone();
    let playlists_ui = playlists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<PlaylistDetail>();
        let header = to_slint_playlist_row(&detail);
        g.set_playlist(header);
        // The sort, an empty needle, no selection and the playlist's tracks, so the user lands on
        // the full track set rather than a stale needle from the previous detail.
        tracks.seat(&state, &playlists_ui);
        // Set the page's enter direction before the `on_applied` hook can flip
        // `Nav.selected-index`, so a cross-section arrival's new page samples it on first paint.
        // Inert on a same-page open, whose body reads a fixed `below`.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_playlist_id(clamp_i64_to_i32(playlist_id));
        // Run after `playlist-id` is set so any global writes the hook performs share this tick
        // with the detail flip — the router then never sees the Playlists grid.
        on_applied(&ui);
        // The two globals six heroes share, written last because their gate is the **live** tab
        // rather than the `section_active` shadow, which the `SectionActiveGate` only updates next
        // frame. Read before the hook above, it answers for the tab being *left*.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Playlists);
        crate::ui::hero_chips::publish_playlist(&ui, &detail, fold, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        // Record a browser-style history entry — see `albums::detail::open_album_with`.
        crate::ui::nav_history::record_current(&ui);
        // Reseat the page's shared filter box, which the clear above doesn't reach — same
        // reasoning, and same closure position, as `albums::detail::open_album_with`.
        ui.global::<melodia_ui::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-read an already-open playlist after a library change or a CRUD operation, preserving the
/// user's current sort, filter and selection. Same shape as `albums::detail::refresh_detail`.
pub async fn refresh_detail(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    playlist_id: i64,
) -> AppResult<()> {
    track_rows::reload(state, playlists_ui).await?;
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

/// Optimistically move the dragged row in the keyset, *before* the DB write lands, returning
/// whether anything moved. Whatever the write answers, the caller asks the database for the order
/// it holds once it returns.
pub fn apply_optimistic_reorder(
    ui: &AppWindow,
    playlists_ui: &PlaylistsUi,
    from: usize,
    to: usize,
) -> bool {
    let g = ui.global::<PlaylistDetail>();
    let field = g.get_sort_field();
    let dir = g.get_sort_dir();
    // Asked here as well as in `reorder-enabled`, this being where the display indices reach the
    // keyset. The filter term is the sharp one: filtered, the list is a subset of the playlist, so
    // `from` names a different track — and the write still lands, a filtered index being in range.
    // The list on show is asked too: it takes a sort or a needle a query after the page does, and
    // an overtaken open can leave another playlist's rows up under this one's id.
    if !is_manual_order(&field, &dir)
        || playlists_ui.detail.is_filtered()
        || !playlists_ui.detail.shows_own_order()
    {
        return false;
    }
    let rows = g.get_tracks();
    TrackRows::installed(&rows, "PlaylistDetail.tracks")
        .is_some_and(|model| model.move_row(from, to))
}

/// Reopen the playlist that was visible at the last shutdown, if any: now if the launch shows the
/// Playlists tab, otherwise on that tab's first enter.
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
        // Above, not a drill-in slide: nobody navigated into this playlist, it is their last view.
        if let Err(e) = open_playlist(&s, &pu, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!("playlists::restore_detail open_playlist({id}): {}", describe(&e));
        }
        // Lowered however it went, and behind `open_playlist`'s own hop so the id is already in:
        // a playlist deleted since the last session owes the grid back rather than an empty body.
        pu.section.end_restore();
        // A grid handed back is warmed before the hop below shows it; a reopen turns this away.
        crate::ui::grid_prewarm::prewarm_off_thread(&pu, PlaylistsUi::prewarm_visible_covers).await;
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<PlaylistDetail>().set_restoring(false);
        });
    });
}
