//! Recently-Played track list: fetch (recency order), in-memory filter +
//! column re-sort, header stats, and model apply.
//!
//! Unlike Favorites (which re-queries with a DB `ORDER BY` on every sort), the
//! recency set is fetched once and its membership is fixed. Sorting and
//! filtering re-walk the cached `tracks_all` entirely in memory — the default
//! [`super::RECENCY_SORT`] keeps the fetch order; a real column field routes
//! through the shared `sort_track_rows_by`.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::{RECENCY_SORT, RecentlyPlayedUi};
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::tracks::{PreparedTrackRow, finish_track_list_row};
use crate::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

/// Read-and-return the active sort (Rust cache is the source of truth).
pub fn current_sort(rp_ui: &RecentlyPlayedUi) -> ViewSort {
    rp_ui.state().sort.lock().clone()
}

/// Read-and-return the active filter string.
pub fn current_filter(rp_ui: &RecentlyPlayedUi) -> String {
    rp_ui.state().filter.lock().clone()
}

/// Update the cached sort. Callers follow with `apply_filtered_tracks`
/// (in-memory re-sort — no re-fetch) + a `set_view_sort` persist.
pub fn set_sort(rp_ui: &RecentlyPlayedUi, field: String, dir: SortDir) {
    *rp_ui.state().sort.lock() = ViewSort { field, dir };
}

/// Update the cached filter string.
pub fn set_filter(rp_ui: &RecentlyPlayedUi, filter: String) {
    *rp_ui.state().filter.lock() = filter;
}

/// Fetch the 200 most-recently-played tracks, cache them, push the header
/// stats, then apply the in-memory filter/sort into the Slint model. Runs on a
/// tokio worker; the model write hops to the UI thread.
pub async fn refresh_tracks(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
) -> AppResult<()> {
    let rows = library::recently_played::get_recently_played(state).await?;

    // Prewarm the row covers off-thread before the first model apply — the
    // `!Send` cover lookup in `finish_track_list_row` runs on the UI thread,
    // so a cold cache would otherwise pay one synchronous decode per unique
    // cover at paint time. `prewarm` dedupes its input.
    let cover_paths: Vec<PathBuf> = rows
        .iter()
        .filter_map(|r| r.artwork_path.as_deref().map(PathBuf::from))
        .collect();
    if !cover_paths.is_empty() {
        let thumbs = rp_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&cover_paths)).await;
    }

    // Hero: count + total duration over the full (unfiltered) recency set, and
    // the up-to-4 most-recently-played distinct covers for the mosaic. Stats +
    // mosaic paths push immediately; the CPU-bound blur composition is kicked
    // off-thread so it never delays the track-list paint.
    let count = i32::try_from(rows.len()).unwrap_or(i32::MAX);
    let total_ms: i64 = rows.iter().map(|r| r.duration_ms).sum();
    let mosaic_paths = super::hero::mosaic_paths_from(&rows, 4);
    super::hero::push_hero_stats(count, total_ms, &mosaic_paths, weak);
    // Only recompose the hero blur when the mosaic covers actually changed. A
    // played-track refresh (or an in-view favorite/rating toggle, which also
    // bumps a subscribed channel) usually yields the same top-4 covers, and
    // re-decoding + re-blurring them is wasted blocking-pool work. Reset on
    // section-leave so a genuine re-enter recomposes.
    let blur_changed = {
        let mut last = rp_ui.state().last_mosaic_paths.lock();
        let changed = *last != mosaic_paths;
        if changed {
            last.clone_from(&mosaic_paths);
        }
        changed
    };
    if blur_changed {
        let st = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        state.runtime.spawn(async move {
            super::hero::refresh_blur(&st, &ru, mosaic_paths, &weak, /* animate */ true).await;
        });
    }

    *rp_ui.state().tracks_all.lock() = rows;

    apply_filtered_tracks(rp_ui, weak);
    Ok(())
}

/// Re-walk the cached `tracks_all` through the active filter + column sort and
/// push the result into `RecentlyPlayed.tracks`. Cheap — entirely in memory.
/// Existing selection is re-stamped so a filter/sort change doesn't visually
/// drop the user's selection.
pub fn apply_filtered_tracks(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let needle = current_filter(rp_ui).to_lowercase();
    let sort = current_sort(rp_ui);

    let prepared: Vec<PreparedTrackRow> = {
        let all = rp_ui.state().tracks_all.lock();
        let mut filtered: Vec<&RsTrackListRow> = all
            .iter()
            .filter(|r| crate::ui::detail_filter::track_matches(r, &needle))
            .collect();
        if sort.field != RECENCY_SORT {
            let dir = match sort.dir {
                SortDir::Asc => "asc",
                SortDir::Desc => "desc",
            };
            crate::ui::track_sort::sort_track_rows_by(
                &mut filtered,
                &sort.field,
                dir,
                |r| *r,
                |r| r.title.to_lowercase(),
            );
        }
        filtered
            .into_iter()
            .map(crate::ui::tracks::prepare_track_list_row)
            .collect()
    };
    let filtered_count = i32::try_from(prepared.len()).unwrap_or(i32::MAX);

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<RecentlyPlayed>();
        let model = g.get_tracks();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
            log::warn!("RecentlyPlayed.tracks: VecModel<TrackListRow> downcast failed");
            return;
        };
        let mut rendered: Vec<UiTrackListRow> =
            prepared.into_iter().map(finish_track_list_row).collect();
        super::selection::restamp_rows(&g, &mut rendered);
        crate::ui::model_diff::apply_rows_keyed(vec, rendered, |r| r.id);
        g.set_filtered_count(filtered_count);
    });
}

/// Flip `is_favorite` on a single row in the Slint `VecModel`. Only touches the
/// affected row — scroll position and neighbouring rows stay put. Recency
/// membership is independent of the favorite flag, so an in-place patch is
/// always correct here (no re-fetch, no full-list rebuild). Mirrors
/// [`crate::ui::tracks::apply_row_favorite`].
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::model_patch::patch_track_row_by_id(
            &ui.global::<RecentlyPlayed>().get_tracks(),
            id,
            |r| r.is_favorite = fav,
        );
    });
}

/// Set `rating` on a single row in the Slint `VecModel` — the star-rating
/// analogue of [`apply_row_favorite`]. These lists have no rating column and no
/// in-table rating sort, so the row never moves; an in-place patch is always
/// correct.
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::model_patch::patch_track_row_by_id(
            &ui.global::<RecentlyPlayed>().get_tracks(),
            id,
            |r| r.rating = rating,
        );
    });
}
