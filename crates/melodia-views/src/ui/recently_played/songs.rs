//! The Songs tab: the capped recency set, fetched once per refresh and filtered in memory in its
//! own order.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, VecModel, Weak};

use super::state::SongsTotals;
use super::{RecentlyPlayedTab, RecentlyPlayedUi};
use crate::ui::list_selection;
use crate::ui::row_match;
use crate::ui::util::{self, clamp_i64_to_i32, len_as_i32};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::AppResult;
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

pub fn current_filter(rp_ui: &RecentlyPlayedUi) -> Needle {
    rp_ui.state().filter.lock().clone()
}

/// Folded on the way in, so the list and the Most Played grid share one needle.
pub fn set_filter(rp_ui: &RecentlyPlayedUi, filter: &str) {
    *rp_ui.state().filter.lock() = fold_needle(filter);
}

/// Fetches the recency set, stores it with the band's facts, then writes the filtered rows.
pub async fn refresh_tracks(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
) -> AppResult<()> {
    let rows = library::recently_played::get_recently_played(&state.db).await?;

    // A leave landing mid-query has already cleared the cache and the model.
    if !rp_ui.section_active() {
        return Ok(());
    }

    // Warmed before the first write, a mounted row resolving its cover on the UI thread. The rows
    // are in recency order, so the prefix the cap keeps is the right one.
    let cover_paths: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        rows.iter().map(|r| r.artwork_path.as_deref()),
        rp_ui.cover_thumbs.capacity(),
    );
    if !cover_paths.is_empty() {
        let thumbs = rp_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&cover_paths)).await;
    }

    // Asked again, the prewarm being an `.await`.
    if !rp_ui.section_active() {
        return Ok(());
    }

    // The band's facts, over the unfiltered set.
    let totals = SongsTotals {
        tracks: len_as_i32(rows.len()),
        duration_ms: rows.iter().map(|r| r.duration_ms).sum(),
    };
    let fold = crate::ui::hero_folds::fold_tracks(&rows);
    let mosaic_paths = super::hero::mosaic_paths_from(&rows, 4);

    // Stored before the push below, so the band can't publish a fold this hasn't landed.
    {
        let _gate = rp_ui.gate();
        *rp_ui.state().songs_totals.lock() = totals;
        *rp_ui.state().songs_fold.lock() = fold;
        *rp_ui.state().tracks_all.lock() = rows;
    }

    super::hero::push_hero_stats(totals.tracks, rp_ui, weak);
    // A refresh usually yields the same top four. The matching claim is `publish_hero_artwork`'s,
    // for `MosaicGuard`'s reason.
    if rp_ui.state().last_mosaic_paths.is_stale(&mosaic_paths) {
        let st = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        state.runtime.spawn(async move {
            super::hero::refresh_artwork(&st, &ru, mosaic_paths, &weak, /* animate */ true).await;
        });
    }

    apply_filtered_tracks(rp_ui, weak);
    Ok(())
}

/// Re-walks the cache through the active filter into `RecentlyPlayed.tracks`, keeping the
/// selection. An unmounted Songs tab is refused twice: before the build, to skip its cost, and at
/// the write with the hidden page, a leave or a pick landing while the post is in flight.
pub fn apply_filtered_tracks(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let Some(rows) = build_filtered_tracks(rp_ui) else {
        return;
    };

    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        write_filtered_tracks(&ui, &rp_ui, rows);
    });
}

/// [`apply_filtered_tracks`] without the event-loop hop, for a tab pick: a hop lets a redraw
/// paint the list's headers over the emptied model first.
pub fn apply_filtered_tracks_now(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    if let Some(rows) = build_filtered_tracks(rp_ui) {
        write_filtered_tracks(ui, rp_ui, rows);
    }
}

fn build_filtered_tracks(rp_ui: &RecentlyPlayedUi) -> Option<Vec<UiTrackListRow>> {
    if rp_ui.active_tab() != RecentlyPlayedTab::Songs {
        return None;
    }
    let needle = current_filter(rp_ui);
    let all = rp_ui.state().tracks_all.lock();
    Some(crate::ui::tracks::to_slint_track_list_rows(
        all.iter().filter(|r| row_match::track_matches(r, &needle)),
    ))
}

fn write_filtered_tracks(ui: &AppWindow, rp_ui: &RecentlyPlayedUi, mut rows: Vec<UiTrackListRow>) {
    if !rp_ui.section_active() || rp_ui.active_tab() != RecentlyPlayedTab::Songs {
        return;
    }
    let g = ui.global::<RecentlyPlayed>();
    let model = g.get_tracks();
    let Some(vec) = util::installed::<VecModel<UiTrackListRow>, _>(&model, "RecentlyPlayed.tracks")
    else {
        return;
    };
    list_selection::keep_selection_within(&g, || {
        rp_ui.state().tracks_all.lock().iter().map(|row| clamp_i64_to_i32(row.id)).collect()
    });
    list_selection::restamp_row_selection(&g, &mut rows);
    crate::ui::model_diff::apply_rows_keyed(vec, rows, |r| r.id);
}

/// Patches the one row in place, recency membership not depending on the flag.
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::model_patch::patch_track_row_by_id(
            &ui.global::<RecentlyPlayed>().get_tracks(),
            id,
            |r| r.is_favorite = fav,
        );
    });
}

/// [`apply_row_favorite`]'s star-rating twin.
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::model_patch::patch_track_row_by_id(
            &ui.global::<RecentlyPlayed>().get_tracks(),
            id,
            |r| r.rating = rating,
        );
    });
}
