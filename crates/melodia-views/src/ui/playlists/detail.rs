//! Playlist Detail's own half: fetch, artwork pair decode, open, refresh and the startup seed,
//! plus a `"position"` sort that rebuilds the display order from the canonical position-order
//! cache instead of re-fetching. What the four details share is [`crate::ui::track_detail`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, SharedString, Weak};

use super::{PlaylistsUi, to_slint_playlist_row};
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_view::{apply_detail_artwork, resolve_view_sort};
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::track_detail::{DetailCache, TrackDetail, filter, selection};
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_rows_by;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::playlist::PlaylistStats;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, NavEnterFrom, PlaylistDetail, TrackListRow as UiTrackListRow};

/// The playlist's own curated order. Synthetic — no column header asks for it,
/// which is why the sort cycle has to hand it back (`next_sort_with_natural`).
pub const POSITION_FIELD: &str = "position";

impl TrackDetail for PlaylistsUi {
    type Global = PlaylistDetail<'static>;

    const NATURAL_SORT: Option<&'static str> = Some(POSITION_FIELD);

    fn cache(&self) -> &DetailCache {
        &self.detail.cache
    }

    fn resort(&self, g: &Self::Global) {
        resort_detail(g, self);
    }

    fn refilter(&self, g: &Self::Global) {
        apply_filtered_detail(g, self);
    }

    fn clear_detail(&self) {
        self.detail.cache.clear();
        self.detail.position_order.lock().clear();
        crate::ui::window_chrome::set_current_playlist_id(-1);
    }
}

/// Whether the rows on screen are in canonical position order, ascending.
///
/// The drag hands over *display* indices and [`apply_optimistic_reorder`] writes them straight
/// into `position_order`, so this is the whole precondition for that mapping being the identity.
/// The direction half is the easy one to miss: [`sort_playlist_tracks`] reverses on `"desc"`,
/// which would make every drag write the inverse permutation.
pub(super) fn is_manual_order(field: &str, dir: &str) -> bool {
    field == POSITION_FIELD && dir != "desc"
}

async fn fetch_playlist_detail(
    state: &AppState,
    playlists_ui: &PlaylistsUi,
    playlist_id: i64,
) -> AppResult<(PlaylistStats, Vec<RsTrackListRow>)> {
    let mut detail = library::playlists::get_playlist_detail(&state.db, playlist_id).await?;
    let tracks = if detail.is_smart {
        // Smart playlists have no `playlist_items` rows — resolve membership live from the stored
        // criteria, and derive the header stats from the resolved set, the junction-maintained
        // `track_count`/`total_duration_ms` staying 0 for a virtual playlist.
        let criteria = melodia_core::entities::smart_criteria::SmartCriteria::from_json_opt(
            detail.smart_criteria.as_deref(),
        );
        let rows = library::smart_playlists::evaluate(&state.db, &criteria).await?;
        detail.track_count = len_as_i32(rows.len());
        detail.total_duration_ms = rows.iter().map(|t| t.duration_ms).sum();
        rows
    } else {
        library::playlists::get_playlist_tracks(&state.db, playlist_id).await?
    };

    let track_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        tracks.iter().map(|t| t.artwork_path.as_deref()),
        playlists_ui.cover_thumbs.capacity(),
    );
    if !track_covers.is_empty() {
        let row_thumbs = playlists_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || {
            row_thumbs.prewarm(&track_covers);
        })
        .await;
    }
    Ok((detail, tracks))
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
    let (detail, mut tracks) = fetch_playlist_detail(state, playlists_ui, playlist_id).await?;

    // `fetch_playlist_detail` returns tracks in playlist position order — capture that as the
    // canonical `position_order` before applying the persisted detail sort. `position` is the
    // fresh-install default; a persisted non-`position` sort is restored across restarts.
    let position_order: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let (sort_field, sort_dir) = resolve_view_sort(state, view_id::PLAYLIST_DETAIL, POSITION_FIELD);
    sort_playlist_tracks(&mut tracks, &position_order, &sort_field, &sort_dir);

    let pair = decode_detail_pair(
        state,
        playlists_ui.detail_artwork.clone(),
        detail.thumbnail_path.clone(),
    )
    .await;

    let ui_tracks: Vec<UiTrackListRow> = crate::ui::tracks::to_slint_track_list_rows(&tracks);

    // Seed both caches before the UI hop so resort / drag-reorder / play-row callbacks firing on
    // the next tick already see consistent state.
    playlists_ui.detail.cache.set_id(playlist_id);
    *playlists_ui.detail.position_order.lock() = position_order;

    // Inform the OS file-drop coalescer that this playlist is the current drop target — used only
    // when the Queue Sheet is closed, the queue taking priority when both are open. A smart
    // playlist's membership is derived, so it registers none and drops fall through to import.
    crate::ui::window_chrome::set_current_playlist_id(if detail.is_smart {
        -1
    } else {
        playlist_id
    });

    // How far the playlist spreads — folded on the worker that fetched it.
    let fold = crate::ui::hero_folds::fold_tracks(&tracks);

    let playlists_ui = playlists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<PlaylistDetail>();
        let header = to_slint_playlist_row(&detail);
        g.set_playlist(header);
        filter::install_tracks(&g, ui_tracks);
        selection::reset(&g, &playlists_ui.detail.cache);
        // Fresh open clears the filter so the user lands on the full track set, not a stale needle
        // from the previous detail; `seat_unfiltered` below clears the Rust half.
        g.set_filter(SharedString::default());
        g.set_sort_field(SharedString::from(sort_field.as_str()));
        g.set_sort_dir(SharedString::from(sort_dir.as_str()));
        // Set the page's enter direction before the `on_applied` hook can flip
        // `Nav.selected-index`, so a cross-section arrival's new page samples it on first paint.
        // Inert on a same-page open, whose body reads a fixed `below`.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_playlist_id(clamp_i64_to_i32(playlist_id));
        playlists_ui.detail.cache.seat_unfiltered(tracks);
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

/// Re-fetch an already-open playlist's header + tracks after a library change or a CRUD operation,
/// preserving the user's current sort column and selection. Same shape as
/// `albums::detail::refresh_detail`, plus the canonical position-order cache is refreshed too.
pub async fn refresh_detail(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    playlist_id: i64,
) -> AppResult<()> {
    let (detail, mut tracks) = fetch_playlist_detail(state, playlists_ui, playlist_id).await?;

    let pair = decode_detail_pair(
        state,
        playlists_ui.detail_artwork.clone(),
        detail.thumbnail_path.clone(),
    )
    .await;

    // The DB query returns tracks in position order, so capture that before permuting `tracks` to
    // the user's chosen sort.
    let position_order_snapshot: Vec<i64> = tracks.iter().map(|t| t.id).collect();

    let fold = crate::ui::hero_folds::fold_tracks(&tracks);

    let playlists_ui = playlists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<PlaylistDetail>();
        if i64::from(g.get_playlist_id()) != playlist_id {
            return;
        }

        let field = g.get_sort_field().to_string();
        let dir = g.get_sort_dir().to_string();
        sort_playlist_tracks(&mut tracks, &position_order_snapshot, &field, &dir);

        g.set_playlist(to_slint_playlist_row(&detail));
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Playlists);
        crate::ui::hero_chips::publish_playlist(&ui, &detail, fold, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, on_screen);

        // Prune `selected-ids` to ids that still exist, then let the shared filter pass
        // re-derive the displayed cache and the model from the canonical set. It diffs, so a
        // scan that touched unrelated files writes back only the rows whose content moved and
        // keeps both the shift-range anchor and an in-flight drag.
        selection::prune_selection_to(&g, &tracks);
        playlists_ui.detail.cache.set_all_tracks(tracks);
        *playlists_ui.detail.position_order.lock() = position_order_snapshot;
        apply_filtered_detail(&g, &playlists_ui);
    });
    Ok(())
}

/// Sort both cached track lists to `field` / `dir` and permute the visible `tracks` model to
/// match. `"position"` rebuilds from the canonical position-order cache via a `HashMap` lookup per
/// row; any other field falls through to the shared `sort_track_rows_by` helper.
///
/// Reconciling the selection is the caller's, and only [`resort_detail`] owes it: a permutation
/// carries each row's `selected` with it, so the drag path has nothing to reconcile.
fn reapply_order(g: &PlaylistDetail<'_>, playlists_ui: &PlaylistsUi, field: &str, dir: &str) {
    let order = {
        let position_order = playlists_ui.detail.position_order.lock();
        playlists_ui
            .detail
            .cache
            .sort_both(|rows| sort_playlist_tracks(rows, &position_order, field, dir))
    };
    crate::ui::model_diff::permute_rows_by_id(&g.get_tracks(), &order, |r| r.id);
}

/// Re-sort the cached detail tracks to the current `PlaylistDetail` sort state,
/// then reorder the existing `tracks` model rows to match.
fn resort_detail(g: &PlaylistDetail<'_>, playlists_ui: &PlaylistsUi) {
    let field = g.get_sort_field();
    let dir = g.get_sort_dir();
    reapply_order(g, playlists_ui, &field, &dir);
    selection::apply_selection_to_rows(g, &playlists_ui.detail.cache);
}

/// Optimistically reorder the cached detail state for a drag-and-drop commit *before* the DB write
/// lands. Returns the position order it replaced so the caller can roll back on a DB error, or
/// `None` when nothing was touched.
pub fn apply_optimistic_reorder(
    ui: &AppWindow,
    playlists_ui: &PlaylistsUi,
    from: usize,
    to: usize,
) -> Option<Vec<i64>> {
    let g = ui.global::<PlaylistDetail>();
    let field = g.get_sort_field();
    let dir = g.get_sort_dir();
    // Asked here as well as in `reorder-enabled`, this being where the display indices reach the
    // cache. The filter term is the sharp one: filtered, `tracks` is a subset of the canonical
    // `position_order`, so `from` names a different track — and the write still lands, a filtered
    // index being in range.
    if !is_manual_order(&field, &dir) || playlists_ui.detail.cache.is_filtered() {
        return None;
    }

    // Snapshot for rollback BEFORE we mutate anything.
    let saved = playlists_ui.detail.position_order.lock().clone();

    {
        let mut pos = playlists_ui.detail.position_order.lock();
        if from >= pos.len() || to > pos.len() {
            return None;
        }
        let id = pos.remove(from);
        let insert_at = to.min(pos.len());
        pos.insert(insert_at, id);
    }

    // The guard above rules out a filter, so the displayed `tracks` cache equals the canonical
    // `all_tracks` here and both re-sort off the same order.
    reapply_order(&g, playlists_ui, &field, &dir);

    Some(saved)
}

/// Put back the position order [`apply_optimistic_reorder`] replaced, when the DB write fails.
///
/// Only the order: the rows are re-sorted as they stand, so a heart, a star, a needle or a
/// refresh landing while the write was in flight survives the rollback.
pub fn rollback_reorder(ui: &AppWindow, playlists_ui: &PlaylistsUi, position_order: Vec<i64>) {
    *playlists_ui.detail.position_order.lock() = position_order;
    resort_detail(&ui.global::<PlaylistDetail>(), playlists_ui);
}

/// The shared filter pass, plus the drag abort a reset owes. UI thread.
fn apply_filtered_detail(g: &PlaylistDetail<'_>, playlists_ui: &PlaylistsUi) {
    let was_reset = filter::apply_filtered_detail(g, &playlists_ui.detail.cache);
    if was_reset {
        // Abort any in-flight drag-reorder: the row indices it was computed against no longer
        // describe the playlist, and the reset destroyed the row instance holding the pointer
        // grab, so it can never clear this state itself. Left set, the source row stays ghosted
        // and the drop line stranded. This is the only detail view that drags, so the abort
        // lives here rather than in the shared pass.
        g.set_drag_source(-1);
        g.set_drop_slot(-1);
    }
}

pub fn seed_detail_from_settings(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
) {
    let Some(id) = library::settings::get_view_state(&state.paths).ok().and_then(|s| {
        s.last_detail_ids.get(crate::ui::track_list_view::view_id::PLAYLIST_DETAIL).copied()
    }) else {
        return;
    };
    // Synchronously, so it is up before `app.show()` — see `PlaylistDetail.restoring`.
    ui.global::<PlaylistDetail>().set_restoring(true);
    playlists_ui.section.begin_restore();
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Above = first-launch fade-down, not a drill-in slide. The user
        // didn't navigate — we're just restoring their last view.
        if let Err(e) = open_playlist(&s, &pu, weak.clone(), id, NavEnterFrom::Above).await {
            log::warn!(
                "playlists::seed_detail_from_settings open_playlist({id}): {}",
                describe(&e)
            );
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

/// Sort `rows` in place by `field` / `dir`. `"position"` rebuilds from
/// the position-order cache (an O(N) `HashMap` lookup per row); any
/// other field falls through to the shared `sort_track_rows_by` helper.
fn sort_playlist_tracks(
    rows: &mut [RsTrackListRow],
    position_order: &[i64],
    field: &str,
    dir: &str,
) {
    if field == POSITION_FIELD {
        let index_of: HashMap<i64, usize> =
            position_order.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        rows.sort_by_cached_key(|r| index_of.get(&r.id).copied().unwrap_or(usize::MAX));
        if dir == "desc" {
            rows.reverse();
        }
        return;
    }
    sort_track_rows_by(rows, field, dir, |r| r, |r| r.title.to_lowercase());
}
