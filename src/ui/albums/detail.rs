//! Album Detail header + track list: fetch, artwork pair decode, re-sort,
//! refresh-preserving, startup seed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::selection::{apply_selection_to_rows, write_selection};
use super::{AlbumsUi, to_slint_album_row};
use crate::entities::album::AlbumStats;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_filter::FilterRefs;
use crate::ui::detail_view::{impl_detail_view_helpers, resolve_view_sort};
use crate::ui::model_patch;
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_list_rows;
use crate::ui::tracks::PreparedTrackRow;
use crate::ui::util::clamp_i64_to_i32;
use crate::{AlbumDetail, AppWindow, NavEnterFrom, TrackListRow as UiTrackListRow};

// `apply_detail_artwork` (cover + hero-blur write) and
// `replace_tracks_model` (in-place `tracks` `VecModel` swap) — see
// `src/ui/detail_view.rs`.
impl_detail_view_helpers!(artwork AlbumDetail);

/// Fetch an album's header + track list and prewarm their cover
/// thumbnails. Shared by [`open_album`] (fresh user open) and
/// [`refresh_detail`] (watcher-driven refresh).
async fn fetch_album_detail(
    state: &AppState,
    albums_ui: &AlbumsUi,
    album_id: i64,
) -> AppResult<(AlbumStats, Vec<RsTrackListRow>)> {
    let detail = library::albums::get_album_detail(state, album_id).await?;
    let tracks = library::albums::get_album_tracks(state, album_id).await?;

    // Prewarm the detail `TrackList`'s 36 px artwork column against the
    // shared row-tier cache. The big header tile + hero blur are handled
    // separately — `decode_detail_pair` (called from `open_album` /
    // `refresh_detail`) decodes that pair into the `detail_artwork` LRU.
    let track_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        tracks.iter().map(|t| t.artwork_path.as_deref()),
        albums_ui.cover_thumbs.capacity(),
    );
    if !track_covers.is_empty() {
        let row_thumbs = albums_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || {
            row_thumbs.prewarm(&track_covers);
        })
        .await;
    }
    Ok((detail, tracks))
}

/// Fetch an album's header + track list, prewarm thumbnails, and populate
/// the `AlbumDetail` global — which flips `album-id >= 0`, swapping the
/// grid for the detail view. Async; the UI write hops back via
/// `upgrade_in_event_loop`. Fresh-open semantics: resets the detail sort
/// to the disc/track-number default and clears any prior selection. The
/// watcher-driven refresh uses [`refresh_detail`] instead, which preserves
/// both.
pub async fn open_album(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_album_with(state, albums_ui, weak, album_id, enter_from, |_ui| {}).await
}

/// Same as [`open_album`] but the caller can hook into the **same**
/// `upgrade_in_event_loop` closure that writes `album-id`. The hook runs
/// as the last statement on the UI thread, after every detail property
/// is set, so a follow-on global write (e.g. flipping `Nav.selected-index`
/// for cross-tab nav from the Artist Detail) lands in the same frame —
/// Slint paints `AlbumDetailBody` directly with no Albums-grid frame in
/// between.
///
/// `enter_from` chooses the `ViewTransition` enter direction for the new
/// `AlbumDetailBody` mount; pass [`NavEnterFrom::Right`] for any user
/// drill-in (same-tab or cross-tab), and [`NavEnterFrom::Below`] for the
/// first-launch seed path so reopening a saved detail feels like a normal
/// app start.
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
    let (detail, mut tracks) = fetch_album_detail(state, albums_ui, album_id).await?;

    // Apply the persisted detail sort (one shared sort for every album —
    // restored across opens and restarts). `track_number` ascending is the
    // fresh-install default.
    let (sort_field, sort_dir) =
        resolve_view_sort(state, view_id::ALBUM_DETAIL, "track_number");
    sort_track_list_rows(&mut tracks, &sort_field, &sort_dir);

    // Decode the `(cover, blur)` pair for the detail header on the
    // `spawn_blocking` pool — one image decode + one box blur. Both halves
    // are derived from a single source decode (see
    // `ui::artwork_cache::ArtworkPair`). The buffers are raw RGB8 so they
    // cross the upcoming `upgrade_in_event_loop` boundary; the UI thread
    // wraps them in `slint::Image` via `Image::from_rgb8`.
    let pair = decode_detail_pair(
        state,
        albums_ui.detail_artwork.clone(),
        detail.artwork_path.clone(),
    )
    .await;

    // Build the `Send` half of every row here on the worker — only the
    // `!Send` cover decode is left for the UI thread, so the click→detail
    // transition no longer hitches on a 100+ track album.
    let prepared: Vec<PreparedTrackRow> =
        tracks.iter().map(crate::ui::tracks::prepare_track_list_row).collect();

    // Folded here rather than inside the `upgrade_in_event_loop` below — this
    // is the worker that already has the rows, and the UI thread has no reason
    // to walk a long album's track list a second time.
    let genre = crate::ui::hero_chips::dominant_genre(&tracks);

    *albums_ui.detail.album_id.lock() = album_id;

    let albums_ui = albums_ui.clone();
    let state_for_history = state.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<AlbumDetail>();
        // UI-thread step: just the cover lookups + the model swap.
        let ui_tracks: Vec<UiTrackListRow> = prepared
            .into_iter()
            .map(crate::ui::tracks::finish_track_list_row)
            .collect();
        let header = to_slint_album_row(&detail);
        g.set_album(header);
        // Off `detail` rather than the row above it — `disc_count` and
        // `is_compilation` are fetched but never reach `AlbumRow`.
        crate::ui::hero_chips::publish_album(
            &ui,
            &detail,
            genre.as_deref(),
            albums_ui.section_active(),
        );
        // Hero blur cross-fades from the previous album; the cover slot
        // is written directly (no fade — the artwork tile itself is
        // covered by the next album's tile in one frame).
        apply_detail_artwork(&ui, &g, pair, /* animate */ true, albums_ui.section_active());
        replace_tracks_model(&g, ui_tracks);
        reset_detail_selection(&g, &albums_ui);
        // Fresh open clears the filter so the user lands on the full
        // track set, not a stale needle from the previous detail.
        // Slint property + Rust cache cleared together.
        g.set_filter(SharedString::from(""));
        albums_ui.detail.filter.lock().clear();
        g.set_sort_field(SharedString::from(sort_field.as_str()));
        g.set_sort_dir(SharedString::from(sort_dir.as_str()));
        // Set the view-transition direction before the property writes
        // that flip the `if` branch. Caller-supplied so the seed path
        // can pass `Below` (normal app-start fade) instead of `Right`
        // (drill-in slide). Same UI-thread tick as the `album-id` flip
        // and any `on_applied` Nav write, so the new `ViewTransition`
        // samples the right direction on first paint.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_album_id(clamp_i64_to_i32(album_id));
        // Fresh open: no filter, so the displayed cache equals the
        // canonical full set.
        albums_ui.detail.all_tracks.lock().clone_from(&tracks);
        *albums_ui.detail.tracks.lock() = tracks;
        // Run after `album-id` is set so any global writes the hook
        // performs (Nav.selected-index for cross-tab nav, …) land in
        // the same UI-thread tick as the detail flip.
        on_applied(&ui);
        // Record a browser-style history entry. Cross-tab `on_applied`
        // may have already flipped `Nav.selected-index`, so reading it
        // here gives the post-flip section. Mouse-4/Mouse-5 walks back
        // and forward through these entries. No-op while a replay is
        // in flight (the replay's own writes set `suppress`).
        crate::ui::nav_history::record_current(&state_for_history, &ui);
    });
    Ok(())
}

/// Re-fetch an already-open album's header + tracks after a library
/// change, **preserving** the user's current sort column and selection.
///
/// Distinct from [`open_album`]'s fresh-open semantics: the library-changed
/// subscriber fires on every watcher / scan tick (a 2 s debounce during any
/// scan), so it must not silently reset the sort or wipe a multi-selection.
/// The track-model swap is skipped entirely when this album's track id
/// slice — under the current sort — is unchanged, which is the common case
/// when a scan touched unrelated files. Bails if the detail view was closed
/// or navigated away while the fetch was in flight.
pub async fn refresh_detail(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
    album_id: i64,
) -> AppResult<()> {
    let (detail, mut tracks) = fetch_album_detail(state, albums_ui, album_id).await?;

    // Re-decode the `(cover, blur)` pair — the artwork may have changed
    // (cover swap, replace-on-disk). `get_or_decode` is a cache hit when
    // the path is unchanged, so this is cheap in the steady state.
    let pair = decode_detail_pair(
        state,
        albums_ui.detail_artwork.clone(),
        detail.artwork_path.clone(),
    )
    .await;

    let genre = crate::ui::hero_chips::dominant_genre(&tracks);

    let albums_ui = albums_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<AlbumDetail>();
        // The detail view was closed (or switched to another album) while
        // the fetch was in flight — drop this stale refresh.
        if i64::from(g.get_album_id()) != album_id {
            return;
        }

        // Apply the user's *current* sort to the freshly-fetched rows.
        let field = g.get_sort_field().to_string();
        let dir = g.get_sort_dir().to_string();
        sort_track_list_rows(&mut tracks, &field, &dir);

        // Header is one row — always refresh it (artwork / counts may
        // have changed).
        g.set_album(to_slint_album_row(&detail));
        crate::ui::hero_chips::publish_album(
            &ui,
            &detail,
            genre.as_deref(),
            albums_ui.section_active(),
        );
        // No fade on the refresh path — this is the same album, the
        // user did not navigate. Either it's a cache hit (no change) or
        // the cover/blur is being replaced in place.
        apply_detail_artwork(&ui, &g, pair, /* animate */ false, albums_ui.section_active());

        // With an active filter the displayed model is a subset, so the
        // id-slice fast path below (which assumes an unfiltered model)
        // would drop the needle. Route the swap through
        // `apply_filtered_detail` instead so the filter survives.
        if !albums_ui.detail.filter.lock().is_empty() {
            let valid: std::collections::HashSet<i32> =
                tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect();
            let pruned: Vec<i32> =
                g.get_selected_ids().iter().filter(|id| valid.contains(id)).collect();
            write_selection(&g, pruned);
            // Refresh the canonical full set; `apply_filtered_detail`
            // re-derives the displayed `tracks` cache + model from it.
            *albums_ui.detail.all_tracks.lock() = tracks;
            apply_filtered_detail(&ui, &albums_ui);
            return;
        }

        // Take the cheap in-place path when the visible id slice is
        // unchanged — the common case when a scan touched unrelated files,
        // or edited a track already on screen without reordering it.
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
            // Id slice + order unchanged, so every row's `selected` flag is
            // still valid — but row *content* may have changed (a tag edit,
            // a favourite toggle from elsewhere). Rebuild each row, carry
            // the existing `selected` flag over, and write back only the
            // rows whose content actually differs — O(changed), not O(rows).
            let model = g.get_tracks();
            if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
                for (i, t) in tracks.iter().enumerate() {
                    let Some(old) = vm.row_data(i) else { continue };
                    let mut fresh = crate::ui::tracks::to_slint_track_list_row(t);
                    // Selection is unchanged on this branch — keep it.
                    fresh.selected = old.selected;
                    if fresh != old {
                        vm.set_row_data(i, fresh);
                    }
                }
            }
            // No filter on this path — displayed cache equals canonical.
            albums_ui.detail.all_tracks.lock().clone_from(&tracks);
            *albums_ui.detail.tracks.lock() = tracks;
        } else {
            let ui_tracks: Vec<UiTrackListRow> = tracks
                .iter()
                .map(crate::ui::tracks::to_slint_track_list_row)
                .collect();
            replace_tracks_model(&g, ui_tracks);
            // The fresh rows all carry `selected: false`, so the `applied`
            // shadow must be reset to match *before* re-applying — and the
            // Rust-side cache must already hold the new (sorted) tracks so
            // `apply_selection_to_rows` can resolve ids → row indices.
            albums_ui.detail.applied_selection.lock().clear();
            // No filter on this path — displayed cache equals canonical.
            albums_ui.detail.all_tracks.lock().clone_from(&tracks);
            *albums_ui.detail.tracks.lock() = tracks;
            // Indices shifted — prune the selection to surviving ids and
            // reset the anchor, then re-apply to the fresh rows.
            let valid: std::collections::HashSet<i32> = new_ids.iter().copied().collect();
            let pruned: Vec<i32> =
                g.get_selected_ids().iter().filter(|id| valid.contains(id)).collect();
            write_selection(&g, pruned);
            g.set_selection_anchor(-1);
            apply_selection_to_rows(&g, &albums_ui);
        }
    });
    Ok(())
}

/// Re-sort the cached detail tracks to the current `AlbumDetail` sort
/// state, then reorder the existing `tracks` model rows to match. No DB
/// hit, **and no row rebuild** — the `UiTrackListRow` structs already in
/// the Slint model are moved into the new order, so there's zero cover
/// re-decode and zero `SharedString` re-allocation (a header click only
/// changes row *order*, not row *content*). Runs on the UI thread (called
/// from `request-sort`). Selection is preserved — track ids are stable
/// across a re-sort, only the row order changes.
pub fn resort_detail(ui: &AppWindow, albums_ui: &AlbumsUi) {
    let g = ui.global::<AlbumDetail>();
    let field = g.get_sort_field().to_string();
    let dir = g.get_sort_dir().to_string();

    // Sort the Rust caches first — `play-row` / range-select read the
    // displayed `tracks`; `all_tracks` is sorted in lockstep so widening
    // the filter later still yields sorted rows.
    let order: Vec<i32> = {
        sort_track_list_rows(&mut albums_ui.detail.all_tracks.lock(), &field, &dir);
        let mut tracks = albums_ui.detail.tracks.lock();
        sort_track_list_rows(&mut tracks, &field, &dir);
        tracks.iter().map(|t| clamp_i64_to_i32(t.id)).collect()
    };

    // Reorder the existing Slint rows to match. Pulling each row out of
    // the model and re-emitting it in the new order just moves the
    // refcounted struct — no decode, no `format!`, no `SharedString`
    // alloc.
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
    // The reordered structs keep their `selected` flags, so this is a
    // no-op in the steady state (`desired` == `applied`); kept as a cheap
    // defensive re-sync.
    apply_selection_to_rows(&g, albums_ui);
}

/// Clear the detail view's cached state when the user navigates back to
/// the grid. The Slint side has already flipped `album-id` to `-1`.
pub fn clear_detail(albums_ui: &AlbumsUi) {
    *albums_ui.detail.album_id.lock() = -1;
    albums_ui.detail.tracks.lock().clear();
    albums_ui.detail.all_tracks.lock().clear();
    albums_ui.detail.applied_selection.lock().clear();
    albums_ui.detail.filter.lock().clear();
}

/// Update the cached filter needle. The Slint side already mirrors the
/// live text via the `<=>` binding; this Rust mirror lets the re-fetch
/// path (`refresh_detail`) re-apply the filter to fresh data without
/// round-tripping the UI thread for the property read. Always stored
/// folded so the per-keystroke walk doesn't re-fold per row.
pub fn set_filter(albums_ui: &AlbumsUi, needle: &str) {
    *albums_ui.detail.filter.lock() = crate::ui::row_match::fold_needle(needle);
}

/// Re-walk the cached tracks through the current filter and push the
/// filtered Slint model — see [`crate::ui::detail_filter`] for the
/// shared implementation. Runs on the UI thread (filter keystroke /
/// refresh-with-filter).
pub fn apply_filtered_detail(ui: &AppWindow, albums_ui: &AlbumsUi) {
    let g = ui.global::<AlbumDetail>();
    crate::ui::detail_filter::apply_filtered_detail(
        &g,
        &FilterRefs {
            all_tracks: &albums_ui.detail.all_tracks,
            tracks: &albums_ui.detail.tracks,
            applied: &albums_ui.detail.applied_selection,
            filter: &albums_ui.detail.filter,
        },
    );
}

/// Flip `is_favorite` on a single detail row in the Slint `VecModel`.
/// Only touches the affected row — scroll position and neighbours stay
/// put. Mirrors `tracks::apply_row_favorite`.
pub fn apply_detail_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<AlbumDetail>().get_tracks(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// Set `rating` on a single detail row in the Slint `VecModel`. Mirrors
/// [`apply_detail_row_favorite`].
pub fn apply_detail_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<AlbumDetail>().get_tracks(), id, |r| {
            r.rating = rating;
        });
    });
}

/// Reopen the album that was visible in the Album Detail view at the last
/// shutdown, if any. Called once at startup *after* `wire_albums` so the
/// `AlbumDetail` callbacks are already live by the time
/// `open_album`'s `upgrade_in_event_loop` lands. Silently no-ops on a
/// missing / deleted album: `open_album` returns an error, we log it, and
/// `album-id` stays at `-1` so the grid renders.
pub fn seed_detail_from_settings(
    ui: &AppWindow,
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
) {
    let Some(id) = library::settings::get_view_state(state)
        .ok()
        .and_then(|s| {
            s.last_detail_ids
                .get(crate::ui::track_list_view::view_id::ALBUM_DETAIL)
                .copied()
        })
    else {
        return;
    };
    let s = state.clone();
    let au = albums_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Below = first-launch fade-up, not a drill-in slide. The user
        // didn't navigate — we're just restoring their last view.
        if let Err(e) = open_album(&s, &au, weak, id, NavEnterFrom::Below).await {
            log::warn!("albums::seed_detail_from_settings open_album({id}): {e}");
        }
    });
}

/// Clear the `selected-ids` model + anchor without walking the row model —
/// freshly-built rows already carry `selected: false`, so the `applied`
/// shadow is reset to empty to match.
pub(super) fn reset_detail_selection(g: &AlbumDetail, albums_ui: &AlbumsUi) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(-1);
    albums_ui.detail.applied_selection.lock().clear();
}

