//! Recently-Played track list: fetch (recency order), in-memory filter, header
//! stats, and model apply.
//!
//! Unlike Favorites (which re-queries with a DB `ORDER BY` on every sort), the
//! recency set is fetched once and both its membership and its order are fixed
//! — the list is mounted non-sortable. Filtering re-walks the cached
//! `tracks_all` entirely in memory and preserves that order.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::RecentlyPlayedUi;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::tracks::{PreparedTrackRow, finish_track_list_row};
use crate::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

/// Read-and-return the active filter string.
pub fn current_filter(rp_ui: &RecentlyPlayedUi) -> String {
    rp_ui.state().filter.lock().clone()
}

/// Update the cached filter string.
pub fn set_filter(rp_ui: &RecentlyPlayedUi, filter: String) {
    *rp_ui.state().filter.lock() = filter;
}

/// Fetch the 200 most-recently-played tracks, cache them, push the header
/// stats, then apply the in-memory filter into the Slint model. Runs on a
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
    // cover at paint time. The rows are already in recency order, which is
    // the order they paint in, so the prefix surviving the cap is the right
    // one.
    let cover_paths: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        rows.iter().map(|r| r.artwork_path.as_deref()),
        rp_ui.cover_thumbs.capacity(),
    );
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
    // Only recompose the hero blur when the mosaic covers differ from the ones
    // on screen. A played-track refresh (or an in-view favorite/rating toggle,
    // which also bumps a subscribed channel) usually yields the same top-4
    // covers, and re-decoding + re-blurring them is wasted blocking-pool work.
    // Reset on section-leave so a genuine re-enter recomposes. The record is
    // `hero::apply_hero_blur`'s to make, once the buffer is actually published
    // — recorded here it would also cover a compose whose apply is dropped, and
    // that would wedge the hero on the gradient floor for the rest of the
    // session.
    let blur_changed = *rp_ui.state().last_mosaic_paths.lock() != mosaic_paths;
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

/// Re-walk the cached `tracks_all` through the active filter and push the
/// result into `RecentlyPlayed.tracks`, still in recency order. Cheap —
/// entirely in memory. Existing selection is re-stamped so a filter change
/// doesn't visually drop the user's selection.
pub fn apply_filtered_tracks(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let needle = current_filter(rp_ui).to_lowercase();

    let prepared: Vec<PreparedTrackRow> = {
        let all = rp_ui.state().tracks_all.lock();
        all.iter()
            .filter(|r| crate::ui::detail_filter::track_matches(r, &needle))
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
