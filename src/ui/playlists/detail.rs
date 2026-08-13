//! Playlist Detail header + track list: fetch, artwork pair decode,
//! re-sort, refresh-preserving, startup seed. Mirrors `albums::detail`,
//! with a `"position"` sort that rebuilds the display order from the
//! canonical position-order cache instead of re-fetching.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::selection::{apply_selection_to_rows, write_selection};
use super::{PlaylistsUi, to_slint_playlist_row};
use crate::entities::playlist::PlaylistStats;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_filter::FilterRefs;
use crate::ui::detail_view::{impl_detail_view_helpers, resolve_view_sort};
use crate::ui::model_patch;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_rows_by;
use crate::ui::tracks::PreparedTrackRow;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use crate::{AppWindow, NavEnterFrom, PlaylistDetail, TrackListRow as UiTrackListRow};

// `apply_detail_artwork` (cover + hero-blur write) and
// `replace_tracks_model` (in-place `tracks` `VecModel` swap) — see
// `src/ui/detail_view.rs`. Playlist Detail keeps its own position-aware
// `sort_playlist_tracks` below.
impl_detail_view_helpers!(artwork PlaylistDetail);

/// The playlist's own curated order. Synthetic — no column header asks for it,
/// which is why the sort cycle has to hand it back (`next_sort_with_natural`).
pub const POSITION_FIELD: &str = "position";

/// Whether the rows on screen are in canonical position order, ascending.
///
/// The drag hands over *display* indices and [`apply_optimistic_reorder`] writes
/// them straight into `position_order`, so this is the whole precondition for
/// that mapping being the identity. The direction half is the easy one to miss:
/// [`sort_playlist_tracks`] reverses on `"desc"`, which would make every drag
/// write the inverse permutation.
pub(super) fn is_manual_order(field: &str, dir: &str) -> bool {
    field == POSITION_FIELD && dir != "desc"
}

async fn fetch_playlist_detail(
    state: &AppState,
    playlists_ui: &PlaylistsUi,
    playlist_id: i64,
) -> AppResult<(PlaylistStats, Vec<RsTrackListRow>)> {
    let mut detail = library::playlists::get_playlist_detail(state, playlist_id).await?;
    let tracks = if detail.is_smart {
        // Smart playlists have no `playlist_items` rows — resolve membership
        // live from the stored criteria, and derive the header stats from the
        // resolved set (the junction-maintained `track_count`/`total_duration_ms`
        // stay 0 for a virtual playlist).
        let criteria =
            crate::entities::smart_criteria::SmartCriteria::from_json_opt(detail.smart_criteria.as_deref());
        let rows = library::smart_playlists::evaluate(state, &criteria).await?;
        detail.track_count = len_as_i32(rows.len());
        detail.total_duration_ms = rows.iter().map(|t| t.duration_ms).sum();
        rows
    } else {
        library::playlists::get_playlist_tracks(state, playlist_id).await?
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

/// Fetch a playlist's header + track list and populate the `PlaylistDetail`
/// global — which flips `playlist-id >= 0`, swapping the grid for the detail
/// view. Async; the UI write hops back via `upgrade_in_event_loop`.
/// Fresh-open semantics: restores the persisted detail sort and clears any
/// prior selection. The watcher-driven refresh uses [`refresh_detail`]
/// instead, which preserves both.
pub async fn open_playlist(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    playlist_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_playlist_with(state, playlists_ui, weak, playlist_id, enter_from, |_ui| {}).await
}

/// Same as [`open_playlist`] but the caller can hook into the **same**
/// `upgrade_in_event_loop` closure that writes `playlist-id`. The hook runs
/// after every detail property is set, so a follow-on global write — the tab
/// and section flip a `nav_history` replay owes — lands in the same frame,
/// and Slint paints `PlaylistDetailBody` with no Playlists-grid frame in
/// between. See `albums::detail::open_album_with`, which this mirrors.
///
/// `enter_from` chooses the enter direction for the **page** mount a
/// cross-section arrival produces; `PlaylistDetailBody` itself takes a fixed
/// `below`, so it reaches nothing when `Nav.selected-index` doesn't move in
/// the same tick.
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

    // `fetch_playlist_detail` returns tracks in playlist position order —
    // capture that as the canonical `position_order` before applying the
    // persisted detail sort. `position` is the fresh-install default
    // (the playlist's own curated order); a persisted non-`position` sort
    // is restored across opens and restarts.
    let position_order: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let (sort_field, sort_dir) =
        resolve_view_sort(state, view_id::PLAYLIST_DETAIL, POSITION_FIELD);
    sort_playlist_tracks(&mut tracks, &position_order, &sort_field, &sort_dir);

    let pair = decode_detail_pair(
        state,
        playlists_ui.detail_artwork.clone(),
        detail.thumbnail_path.clone(),
    )
    .await;

    let prepared: Vec<PreparedTrackRow> =
        tracks.iter().map(crate::ui::tracks::prepare_track_list_row).collect();

    // Seed both caches before the UI hop so resort / drag-reorder /
    // play-row callbacks that may fire on the next tick already see
    // consistent state.
    *playlists_ui.detail.playlist_id.lock() = playlist_id;
    *playlists_ui.detail.position_order.lock() = position_order;

    // Inform the OS file-drop coalescer that this playlist is the
    // current drop target — used only when the Queue Sheet is closed
    // (queue takes priority when both are open). Smart playlists can't
    // accept manual file drops (membership is derived), so a smart detail
    // registers no drop target — drops fall through to the library import.
    crate::ui::window_chrome::set_current_playlist_id(if detail.is_smart {
        -1
    } else {
        playlist_id
    });

    // How far the playlist spreads — folded on the worker that fetched it.
    let fold = crate::ui::hero_folds::fold_tracks(&tracks);

    let playlists_ui = playlists_ui.clone();
    let state_for_history = state.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<PlaylistDetail>();
        let ui_tracks: Vec<UiTrackListRow> = prepared
            .into_iter()
            .map(crate::ui::tracks::finish_track_list_row)
            .collect();
        let header = to_slint_playlist_row(&detail);
        g.set_playlist(header);
        replace_tracks_model(&g, ui_tracks);
        reset_detail_selection(&g, &playlists_ui);
        // Fresh open clears the filter so the user lands on the full
        // track set, not a stale needle from the previous detail.
        g.set_filter(SharedString::from(""));
        playlists_ui.detail.filter.lock().clear();
        g.set_sort_field(SharedString::from(sort_field.as_str()));
        g.set_sort_dir(SharedString::from(sort_dir.as_str()));
        // Set the page's enter direction before the `on_applied` hook can flip
        // `Nav.selected-index`, so a cross-section arrival's new page samples it
        // on first paint. Inert on a same-page open, whose body reads a fixed
        // `below` — see `ui::nav_transition`.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_playlist_id(clamp_i64_to_i32(playlist_id));
        // Fresh open: no filter, so the displayed cache equals the
        // canonical full set.
        playlists_ui.detail.all_tracks.lock().clone_from(&tracks);
        *playlists_ui.detail.tracks.lock() = tracks;
        // Run after `playlist-id` is set so any global writes the hook performs
        // (the tab and section a history replay lands) share this tick with the
        // detail flip — the router then never sees the Playlists grid.
        on_applied(&ui);
        // The two globals six heroes share, written last because their gate is the
        // **live** tab rather than the `section_active` shadow, which the
        // `SectionActiveGate` only updates next frame. Read before the hook above
        // it answers for the tab being *left*, and both are dropped outright.
        let on_screen = tab_is_mounted(&ui, MyLibraryTab::Playlists);
        crate::ui::hero_chips::publish_playlist(&ui, &detail, fold, on_screen);
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, on_screen);
        // Record a browser-style history entry — see the matching
        // `record_current` in `albums::detail::open_album_with` for
        // the rationale.
        crate::ui::nav_history::record_current(&state_for_history, &ui);
        // Reseat the page's shared filter box, which the clear above
        // doesn't reach — same reasoning, and same closure position, as
        // `albums::detail::open_album_with`.
        ui.global::<crate::MyLibrary>().invoke_detail_scope_changed();
    });
    Ok(())
}

/// Re-fetch an already-open playlist's header + tracks after a library
/// change (or a CRUD operation like add/remove/rename), preserving the
/// user's current sort column and selection. Same shape as
/// `albums::detail::refresh_detail`, plus the canonical position-order
/// cache is also refreshed.
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

    // The DB query returns tracks in position order, so capture that
    // before we permute `tracks` to the user's chosen sort.
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

        // With an active filter the displayed model is a subset, so the
        // id-slice fast path below (which assumes an unfiltered model)
        // would drop the needle. Route the swap through
        // `apply_filtered_detail` instead so the filter survives.
        if !playlists_ui.detail.filter.lock().is_empty() {
            let valid: std::collections::HashSet<i32> =
                tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect();
            let pruned: Vec<i32> =
                g.get_selected_ids().iter().filter(|id| valid.contains(id)).collect();
            write_selection(&g, pruned);
            // Refresh the canonical full set; `apply_filtered_detail`
            // re-derives the displayed `tracks` cache + model from it.
            *playlists_ui.detail.all_tracks.lock() = tracks;
            *playlists_ui.detail.position_order.lock() = position_order_snapshot;
            apply_filtered_detail(&ui, &playlists_ui);
            return;
        }

        let new_ids: Vec<i32> = tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect();
        let cur_ids: Vec<i32> = {
            let model = g.get_tracks();
            model
                .as_any()
                .downcast_ref::<VecModel<UiTrackListRow>>()
                .map(|vm| {
                    (0..vm.row_count())
                        .filter_map(|i| vm.row_data(i))
                        .map(|r| r.id)
                        .collect()
                })
                .unwrap_or_default()
        };
        if new_ids == cur_ids {
            let model = g.get_tracks();
            if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
                for (i, t) in tracks.iter().enumerate() {
                    let Some(old) = vm.row_data(i) else { continue };
                    let mut fresh = crate::ui::tracks::to_slint_track_list_row(t);
                    fresh.selected = old.selected;
                    if fresh != old {
                        vm.set_row_data(i, fresh);
                    }
                }
            }
            // No filter on this path — displayed cache equals canonical.
            playlists_ui.detail.all_tracks.lock().clone_from(&tracks);
            *playlists_ui.detail.tracks.lock() = tracks;
            *playlists_ui.detail.position_order.lock() = position_order_snapshot;
        } else {
            let ui_tracks: Vec<UiTrackListRow> = tracks
                .iter()
                .map(crate::ui::tracks::to_slint_track_list_row)
                .collect();
            replace_tracks_model(&g, ui_tracks);
            // Abort any in-flight drag-reorder: the row indices it was
            // computed against no longer describe the playlist, and the
            // model swap destroys the row instance holding the pointer
            // grab, so it can never clear this state itself. Left set,
            // the source row stays ghosted and the drop line stranded.
            g.set_drag_source(-1);
            g.set_drop_slot(-1);
            playlists_ui.detail.applied_selection.lock().clear();
            // No filter on this path — displayed cache equals canonical.
            playlists_ui.detail.all_tracks.lock().clone_from(&tracks);
            *playlists_ui.detail.tracks.lock() = tracks;
            *playlists_ui.detail.position_order.lock() = position_order_snapshot;
            let valid: std::collections::HashSet<i32> = new_ids.iter().copied().collect();
            let pruned: Vec<i32> =
                g.get_selected_ids().iter().filter(|id| valid.contains(id)).collect();
            write_selection(&g, pruned);
            g.set_selection_anchor(-1);
            apply_selection_to_rows(&g, &playlists_ui);
        }
    });
    Ok(())
}

/// Sort both cached track lists to `field` / `dir` and permute the visible
/// `tracks` model to match. `"position"` rebuilds from the canonical
/// position-order cache via an O(N) `HashMap` lookup per row; any other field
/// falls through to the shared `sort_track_rows_by` helper.
///
/// Reconciling the selection is the caller's, and only [`resort_detail`] owes
/// it: a permutation carries each row's `selected` with it, so the drag path
/// has nothing to reconcile.
fn reapply_order(g: &PlaylistDetail, playlists_ui: &PlaylistsUi, field: &str, dir: &str) {
    // Sort the canonical full set + the displayed subset in lockstep so
    // `play-row` / range-select read consistent order and widening the
    // filter later still yields sorted rows.
    let order: Vec<i32> = {
        let position_order = playlists_ui.detail.position_order.lock();
        sort_playlist_tracks(
            &mut playlists_ui.detail.all_tracks.lock(),
            &position_order,
            field,
            dir,
        );
        let mut tracks = playlists_ui.detail.tracks.lock();
        sort_playlist_tracks(&mut tracks, &position_order, field, dir);
        tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect()
    };

    let model = g.get_tracks();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        let mut by_id: HashMap<i32, UiTrackListRow> = HashMap::with_capacity(vm.row_count());
        for i in 0..vm.row_count() {
            if let Some(r) = vm.row_data(i) {
                by_id.insert(r.id, r);
            }
        }
        let reordered: Vec<UiTrackListRow> =
            order.iter().filter_map(|id| by_id.remove(id)).collect();
        vm.set_vec(reordered);
    }
}

/// Re-sort the cached detail tracks to the current `PlaylistDetail` sort state,
/// then reorder the existing `tracks` model rows to match.
pub fn resort_detail(ui: &AppWindow, playlists_ui: &PlaylistsUi) {
    let g = ui.global::<PlaylistDetail>();
    let field = g.get_sort_field();
    let dir = g.get_sort_dir();
    reapply_order(&g, playlists_ui, &field, &dir);
    apply_selection_to_rows(&g, playlists_ui);
}

/// Optimistically reorder the cached detail state for a drag-and-drop
/// commit *before* the DB write lands. Mutates `position_order` and the
/// visible `tracks` Vec. Returns the pre-mutation pair so the caller can roll
/// back on a DB error, or `None` when nothing was touched.
pub fn apply_optimistic_reorder(
    ui: &AppWindow,
    playlists_ui: &PlaylistsUi,
    from: usize,
    to: usize,
) -> Option<(Vec<i64>, Vec<RsTrackListRow>)> {
    let g = ui.global::<PlaylistDetail>();
    let field = g.get_sort_field();
    let dir = g.get_sort_dir();
    // Asked here as well as in `reorder-enabled`, this being where the display
    // indices reach the cache. The filter term is the sharp one: filtered,
    // `tracks` is a subset of the canonical `position_order`, so `from` names a
    // different track — and the write still lands, a filtered index being in
    // range. (`is_smart` needs no term: the query refuses an empty item set.)
    if !is_manual_order(&field, &dir) || !playlists_ui.detail.filter.lock().is_empty() {
        return None;
    }

    // Snapshot for rollback BEFORE we mutate anything.
    let saved = {
        let pos = playlists_ui.detail.position_order.lock().clone();
        let tracks = playlists_ui.detail.tracks.lock().clone();
        (pos, tracks)
    };

    {
        let mut pos = playlists_ui.detail.position_order.lock();
        if from >= pos.len() || to > pos.len() {
            return None;
        }
        let id = pos.remove(from);
        let insert_at = to.min(pos.len());
        pos.insert(insert_at, id);
    }

    // The guard above rules out a filter, so the displayed `tracks` cache equals
    // the canonical `all_tracks` here and both re-sort off the same order.
    reapply_order(&g, playlists_ui, &field, &dir);

    Some(saved)
}

/// Roll back the cached state to the snapshot returned by
/// [`apply_optimistic_reorder`]. Called when the DB write fails.
pub fn rollback_reorder(
    ui: &AppWindow,
    playlists_ui: &PlaylistsUi,
    snapshot: (Vec<i64>, Vec<RsTrackListRow>),
) {
    let (pos, tracks) = snapshot;
    *playlists_ui.detail.position_order.lock() = pos;
    // Reorder is disabled while filtered, so the displayed `tracks`
    // cache equals the canonical `all_tracks` — restore both.
    playlists_ui.detail.all_tracks.lock().clone_from(&tracks);
    *playlists_ui.detail.tracks.lock() = tracks;
    // Force a UI rebuild of the visible rows from the rolled-back cache.
    resort_detail(ui, playlists_ui);
}

pub fn clear_detail(playlists_ui: &PlaylistsUi) {
    *playlists_ui.detail.playlist_id.lock() = -1;
    playlists_ui.detail.tracks.lock().clear();
    playlists_ui.detail.all_tracks.lock().clear();
    playlists_ui.detail.position_order.lock().clear();
    playlists_ui.detail.applied_selection.lock().clear();
    playlists_ui.detail.filter.lock().clear();
    crate::ui::window_chrome::set_current_playlist_id(-1);
}

/// Update the cached filter needle. Nothing binds the Slint half to this one —
/// the page's single box reaches nine surfaces through Rust, so
/// `ui::my_library::filter::dispatch` writes both sides. This mirror is what
/// lets the re-fetch path (`refresh_detail`) re-apply the filter to fresh data
/// without round-tripping the UI thread for the property read. Always stored
/// folded so the per-keystroke walk doesn't re-fold per row.
pub fn set_filter(playlists_ui: &PlaylistsUi, needle: &str) {
    *playlists_ui.detail.filter.lock() = crate::ui::row_match::fold_needle(needle);
}

/// Re-walk the cached tracks through the current filter and push the
/// filtered Slint model — see [`crate::ui::detail_filter`] for the
/// shared implementation. Runs on the UI thread (filter keystroke /
/// refresh-with-filter).
pub fn apply_filtered_detail(ui: &AppWindow, playlists_ui: &PlaylistsUi) {
    let g = ui.global::<PlaylistDetail>();
    crate::ui::detail_filter::apply_filtered_detail(
        &g,
        &FilterRefs {
            all_tracks: &playlists_ui.detail.all_tracks,
            tracks: &playlists_ui.detail.tracks,
            applied: &playlists_ui.detail.applied_selection,
            filter: &playlists_ui.detail.filter,
        },
    );
}

pub fn apply_detail_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<PlaylistDetail>().get_tracks(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// Set `rating` on a single detail row in the Slint `VecModel`. Mirrors
/// `apply_detail_row_favorite`.
pub fn apply_detail_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<PlaylistDetail>().get_tracks(), id, |r| {
            r.rating = rating;
        });
    });
}

pub fn seed_detail_from_settings(
    ui: &AppWindow,
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
) {
    let Some(id) = library::settings::get_view_state(state)
        .ok()
        .and_then(|s| {
            s.last_detail_ids
                .get(crate::ui::track_list_view::view_id::PLAYLIST_DETAIL)
                .copied()
        })
    else {
        return;
    };
    let s = state.clone();
    let pu = playlists_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Below = first-launch fade-up, not a drill-in slide. The user
        // didn't navigate — we're just restoring their last view.
        if let Err(e) = open_playlist(&s, &pu, weak, id, NavEnterFrom::Below).await {
            log::warn!("playlists::seed_detail_from_settings open_playlist({id}): {e}");
        }
    });
}

pub(super) fn reset_detail_selection(g: &PlaylistDetail, playlists_ui: &PlaylistsUi) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(-1);
    playlists_ui.detail.applied_selection.lock().clear();
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
        let index_of: HashMap<i64, usize> = position_order
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();
        rows.sort_by_cached_key(|r| index_of.get(&r.id).copied().unwrap_or(usize::MAX));
        if dir == "desc" {
            rows.reverse();
        }
        return;
    }
    sort_track_rows_by(rows, field, dir, |r| r, |r| r.title.to_lowercase());
}
