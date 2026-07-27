//! Artist Detail header + Albums sub-section + track list: fetch, artwork
//! pair decode, re-sort, refresh-preserving, startup seed. Mirrors
//! `src/ui/albums/detail.rs`, plus an `albums` slice that drives the
//! horizontal Albums strip above the all-tracks `TrackList`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use super::selection::{apply_selection_to_rows, write_selection};
use super::{ArtistsUi, to_slint_artist_row};
use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_filter::{restamp_selection, track_matches};
use crate::ui::detail_view::{impl_detail_view_helpers, resolve_view_sort};
use crate::ui::model_patch;
use crate::ui::track_list_view::view_id;
use crate::ui::track_sort::sort_track_list_rows;
use crate::ui::tracks::PreparedTrackRow;
use crate::ui::util::clamp_i64_to_i32;
use crate::{
    AlbumRow as UiAlbumRow, AppWindow, ArtistDetail, NavEnterFrom, TrackListRow as UiTrackListRow,
};

// `apply_detail_artwork` (cover + hero-blur write) and
// `replace_tracks_model` (in-place `tracks` `VecModel` swap) — see
// `src/ui/detail_view.rs`.
impl_detail_view_helpers!(artwork ArtistDetail);

/// Fetch an artist's header + albums sub-section + track list, and prewarm
/// the row-tier covers for the tracks. Shared by [`open_artist`] and
/// [`refresh_detail`].
async fn fetch_artist_detail(
    state: &AppState,
    artists_ui: &ArtistsUi,
    artist_id: i64,
) -> AppResult<(ArtistStats, Vec<AlbumStats>, Vec<RsTrackListRow>)> {
    let detail = library::artists::get_artist_detail(state, artist_id).await?;
    let albums = library::artists::get_artist_albums(state, artist_id).await?;
    let tracks = library::artists::get_artist_tracks(state, artist_id).await?;

    let track_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        tracks.iter().map(|t| t.artwork_path.as_deref()),
    );
    // The Albums strip resolves its cards through the borrowed Albums
    // grid tier (`request-album-cover` → `AlbumsUi::grid_cover`,
    // decode-on-miss on the UI thread). Prewarm those covers alongside
    // the track rows so a detail open with a cold cache doesn't freeze
    // the UI for one full-res decode per album card at first paint.
    let strip_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        albums.iter().map(|a| a.artwork_path.as_deref()),
    );
    if !track_covers.is_empty() || !strip_covers.is_empty() {
        let row_thumbs = artists_ui.cover_thumbs.clone();
        let strip_thumbs = artists_ui.albums_grid_covers.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if !track_covers.is_empty() {
                row_thumbs.prewarm(&track_covers);
            }
            if !strip_covers.is_empty() {
                strip_thumbs.prewarm(&strip_covers);
            }
        })
        .await;
    }
    Ok((detail, albums, tracks))
}

/// Fetch an artist's header + albums + tracks, populate the
/// `ArtistDetail` global, and flip `artist-id >= 0` to swap the grid for
/// the detail view. Fresh-open semantics: resets the detail sort to the
/// default and clears any prior selection.
pub async fn open_artist(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_artist_with(state, artists_ui, weak, artist_id, enter_from, |_ui| {}).await
}

/// Same as [`open_artist`] but the caller can hook into the **same**
/// `upgrade_in_event_loop` closure that writes `artist-id`. The hook
/// runs as the last statement on the UI thread, after every detail
/// property is set, so a follow-on global write (e.g. flipping
/// `Nav.selected-index` for cross-tab nav from Favorites) lands in
/// the same frame — Slint paints `ArtistDetailView` directly with no
/// `ArtistView` (grid) frame in between. Mirrors
/// `albums::open_album_with` exactly.
///
/// `enter_from` chooses the `ViewTransition` enter direction; pass
/// [`NavEnterFrom::Right`] for any user drill-in and
/// [`NavEnterFrom::Below`] for the first-launch seed path.
pub async fn open_artist_with<F>(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
    enter_from: NavEnterFrom,
    on_applied: F,
) -> AppResult<()>
where
    F: FnOnce(&AppWindow) + Send + 'static,
{
    let (detail, albums, mut tracks) =
        fetch_artist_detail(state, artists_ui, artist_id).await?;

    // Apply the persisted detail sort on the worker so the per-row prep
    // below runs in display order; the UI thread can then drop the
    // already-sorted rows straight into the Slint model without an
    // additional reorder pass. One shared sort for every artist —
    // restored across opens and restarts; `album` ascending is the
    // fresh-install default.
    let (sort_field, sort_dir) =
        resolve_view_sort(state, view_id::ARTIST_DETAIL, "album");
    sort_track_list_rows(&mut tracks, &sort_field, &sort_dir);

    let pair = decode_detail_pair(
        state,
        artists_ui.detail_artwork.clone(),
        detail.image_path.clone(),
    )
    .await;

    let prepared: Vec<PreparedTrackRow> =
        tracks.iter().map(crate::ui::tracks::prepare_track_list_row).collect();

    *artists_ui.detail.artist_id.lock() = artist_id;

    let artists_ui = artists_ui.clone();
    let state_for_history = state.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<ArtistDetail>();

        let ui_tracks: Vec<UiTrackListRow> = prepared
            .into_iter()
            .map(crate::ui::tracks::finish_track_list_row)
            .collect();

        let header = to_slint_artist_row(&detail);
        g.set_artist(header);

        // Albums sub-section model — small grid above the track list.
        let album_rows: Vec<UiAlbumRow> = albums
            .iter()
            .map(crate::ui::albums::to_slint_album_row)
            .collect();
        write_albums_model(&g, album_rows);

        apply_detail_artwork(&g, pair, /* animate */ true);
        replace_tracks_model(&g, ui_tracks);
        reset_detail_selection(&g, &artists_ui);
        // Fresh open clears the filter so the user lands on the full
        // tracks + albums set, not a stale filter from the previous
        // detail. Slint property + Rust cache cleared together.
        g.set_filter(SharedString::from(""));
        *artists_ui.detail.filter.lock() = String::new();
        g.set_sort_field(SharedString::from(sort_field.as_str()));
        g.set_sort_dir(SharedString::from(sort_dir.as_str()));
        // Set the view-transition direction before the property writes
        // that flip the `if` branch. Caller-supplied so the seed path
        // can pass `Below` (normal app-start fade) instead of `Right`
        // (drill-in slide). Same UI-thread tick as the `artist-id` flip
        // and any `on_applied` Nav write.
        crate::ui::nav_transition::mark(&ui, enter_from);
        g.set_artist_id(clamp_i64_to_i32(artist_id));
        // Fresh open: no filter, so the displayed cache equals the
        // canonical full set.
        artists_ui.detail.all_tracks.lock().clone_from(&tracks);
        *artists_ui.detail.tracks.lock() = tracks;
        *artists_ui.detail.albums.lock() = albums;
        // Run after `artist-id` is set so any global writes the hook
        // performs (Nav.selected-index for cross-tab nav, …) land in
        // the same UI-thread tick as the detail flip.
        on_applied(&ui);
        // Record a browser-style history entry — see the matching
        // `record_current` in `albums::detail::open_album_with` for
        // the rationale.
        crate::ui::nav_history::record_current(&state_for_history, &ui);
    });
    Ok(())
}

/// Re-fetch an already-open artist's header + albums + tracks after a
/// library change, preserving the user's current sort + selection.
pub async fn refresh_detail(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
    artist_id: i64,
) -> AppResult<()> {
    let (detail, albums, mut tracks) =
        fetch_artist_detail(state, artists_ui, artist_id).await?;

    let pair = decode_detail_pair(
        state,
        artists_ui.detail_artwork.clone(),
        detail.image_path.clone(),
    )
    .await;

    let weak_for_filter = weak.clone();
    let artists_ui_clone = artists_ui.clone();
    let artists_ui = artists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<ArtistDetail>();
        if i64::from(g.get_artist_id()) != artist_id {
            return;
        }

        let field = g.get_sort_field().to_string();
        let dir = g.get_sort_dir().to_string();
        sort_track_list_rows(&mut tracks, &field, &dir);

        g.set_artist(to_slint_artist_row(&detail));
        apply_detail_artwork(&g, pair, /* animate */ false);

        // Refresh the canonical Rust caches (all_tracks + albums) with
        // the freshly-fetched data. The displayed `tracks` cache + the
        // Slint models are then rewritten through `apply_filtered_detail`
        // so the user's existing filter (if any) survives the refresh —
        // no flash of unfiltered rows.
        *artists_ui.detail.all_tracks.lock() = tracks;
        *artists_ui.detail.albums.lock() = albums;
        // Prune `selected-ids` to ids that still exist after the
        // refresh — otherwise the chip + applied-selection drift
        // out of sync with reality.
        let valid: std::collections::HashSet<i32> = artists_ui
            .detail
            .all_tracks
            .lock()
            .iter()
            .map(|t| clamp_i64_to_i32(t.id))
            .collect();
        let pruned: Vec<i32> =
            g.get_selected_ids().iter().filter(|id| valid.contains(id)).collect();
        write_selection(&g, pruned);
        artists_ui.detail.applied_selection.lock().clear();

        // Hand off the model swap to `apply_filtered_detail` so the
        // filter + selection re-stamp pass runs in one place.
        drop(g);
        apply_filtered_detail(&weak_for_filter, &artists_ui_clone);
    });
    Ok(())
}

/// Re-sort the cached detail tracks to the current `ArtistDetail` sort
/// state, then reorder the existing rows to match. No DB hit, no row
/// rebuild. Selection is preserved.
pub fn resort_detail(ui: &AppWindow, artists_ui: &ArtistsUi) {
    let g = ui.global::<ArtistDetail>();
    let field = g.get_sort_field().to_string();
    let dir = g.get_sort_dir().to_string();

    // Sort the canonical full set + the displayed subset in lockstep so
    // `play-row` / range-select read consistent order and widening the
    // filter later still yields sorted rows.
    let order: Vec<i32> = {
        sort_track_list_rows(&mut artists_ui.detail.all_tracks.lock(), &field, &dir);
        let mut tracks = artists_ui.detail.tracks.lock();
        sort_track_list_rows(&mut tracks, &field, &dir);
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
    apply_selection_to_rows(&g, artists_ui);
}

/// Clear cached detail state when the user navigates back to the grid.
pub fn clear_detail(artists_ui: &ArtistsUi) {
    *artists_ui.detail.artist_id.lock() = -1;
    artists_ui.detail.tracks.lock().clear();
    artists_ui.detail.all_tracks.lock().clear();
    artists_ui.detail.albums.lock().clear();
    artists_ui.detail.filter.lock().clear();
    artists_ui.detail.applied_selection.lock().clear();
}

/// Update the cached filter needle. The Slint side already mirrors the
/// live text via `<=>` binding; this Rust mirror lets the re-fetch path
/// (`open_artist_with` / `refresh_detail`) re-apply the filter to fresh
/// data without round-tripping through the UI thread for the property
/// read. Always stored lowercased so the per-keystroke walk doesn't
/// re-lower per row.
pub fn set_filter(artists_ui: &ArtistsUi, needle: &str) {
    *artists_ui.detail.filter.lock() = needle.to_lowercase();
}

/// Re-walk the canonical `all_tracks` + `albums` Vecs through the
/// current filter and push the resulting Slint models. The track match
/// is a case-insensitive substring on title + artist + album; the album
/// match is on album name only. Cheap enough to invoke on every
/// keystroke — the lists are fully in-memory. The filtered track result
/// is stored back into the displayed `tracks` cache so it stays in
/// lockstep with the model (the generic selection/sort logic maps id ↔
/// row-index through that cache). Selection state is re-stamped onto the
/// filtered rows so existing selections survive a filter change.
pub fn apply_filtered_detail(weak: &Weak<AppWindow>, artists_ui: &Arc<ArtistsUi>) {
    let needle = artists_ui.detail.filter.lock().clone();

    let displayed_tracks: Vec<RsTrackListRow> = {
        let all = artists_ui.detail.all_tracks.lock();
        if needle.is_empty() {
            all.clone()
        } else {
            all.iter()
                .filter(|r| track_matches(r, &needle))
                .cloned()
                .collect()
        }
    };

    let filtered_albums: Vec<AlbumStats> = {
        let cache = artists_ui.detail.albums.lock();
        if needle.is_empty() {
            cache.clone()
        } else {
            cache
                .iter()
                .filter(|a| a.name.to_lowercase().contains(&needle))
                .cloned()
                .collect()
        }
    };

    let prepared: Vec<PreparedTrackRow> = displayed_tracks
        .iter()
        .map(crate::ui::tracks::prepare_track_list_row)
        .collect();
    let album_rows: Vec<UiAlbumRow> =
        filtered_albums.iter().map(crate::ui::albums::to_slint_album_row).collect();

    let weak = weak.clone();
    let artists_ui = artists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let g = ui.global::<ArtistDetail>();

        let mut ui_tracks: Vec<UiTrackListRow> = prepared
            .into_iter()
            .map(crate::ui::tracks::finish_track_list_row)
            .collect();
        restamp_selection(&g, &mut ui_tracks);
        // Keep the displayed `tracks` cache in lockstep with the model.
        *artists_ui.detail.tracks.lock() = displayed_tracks;
        replace_tracks_model(&g, ui_tracks);
        write_albums_model(&g, album_rows);
        // The displayed model changed shape — a stale shift-range
        // anchor would now index the wrong row, so drop it.
        // `restamp_selection` already stamped the rows, so the `applied`
        // shadow just mirrors `selected-ids` for the next click diff.
        g.set_selection_anchor(-1);
        *artists_ui.detail.applied_selection.lock() = g.get_selected_ids().iter().collect();
    });
}

/// Flip `is_favorite` on a single detail row in the Slint `VecModel`.
pub fn apply_detail_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<ArtistDetail>().get_tracks(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// Set `rating` on a single detail row in the Slint `VecModel`. Mirrors
/// [`apply_detail_row_favorite`].
pub fn apply_detail_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<ArtistDetail>().get_tracks(), id, |r| {
            r.rating = rating;
        });
    });
}

/// Reopen the artist that was visible at the last shutdown, if any.
pub fn seed_detail_from_settings(
    ui: &AppWindow,
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
) {
    let Some(id) = library::settings::get_view_state(state)
        .ok()
        .and_then(|s| {
            s.last_detail_ids
                .get(crate::ui::track_list_view::view_id::ARTIST_DETAIL)
                .copied()
        })
    else {
        return;
    };
    let s = state.clone();
    let au = artists_ui.clone();
    let weak = ui.as_weak();
    state.runtime.spawn(async move {
        // Below = first-launch fade-up, not a drill-in slide. The user
        // didn't navigate — we're just restoring their last view.
        if let Err(e) = open_artist(&s, &au, weak, id, NavEnterFrom::Below).await {
            log::warn!("artists::seed_detail_from_settings open_artist({id}): {e}");
        }
    });
}

/// Clear the `selected-ids` model + anchor without walking the row model.
pub(super) fn reset_detail_selection(g: &ArtistDetail, artists_ui: &ArtistsUi) {
    write_selection(g, Vec::new());
    g.set_selection_anchor(-1);
    artists_ui.detail.applied_selection.lock().clear();
}

/// Swap the `ArtistDetail.albums` `VecModel` contents in place.
fn write_albums_model(g: &ArtistDetail, rows: Vec<UiAlbumRow>) {
    let model = g.get_albums();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiAlbumRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_albums(ModelRc::from(Rc::new(VecModel::from(rows))));
    }
}
