//! Songs tab: fetch (recency order), in-memory filter, hero stats, and model
//! apply.
//!
//! Unlike Favorites (which re-queries with a DB `ORDER BY` on every sort), the
//! recency set is fetched once and both its membership and its order are fixed
//! — the list is mounted non-sortable. Filtering re-walks the cached
//! `tracks_all` entirely in memory and preserves that order.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::state::SongsTotals;
use super::{RecentlyPlayedTab, RecentlyPlayedUi};
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::row_match::{self, Needle};
use crate::ui::tracks::{PreparedTrackRow, finish_track_list_row};
use crate::ui::util::len_as_i32;
use crate::{AppWindow, RecentlyPlayed, TrackListRow as UiTrackListRow};

/// Read-and-return the active filter needle, folded by [`set_filter`] and ready
/// to hand to a `row_match` predicate.
pub fn current_filter(rp_ui: &RecentlyPlayedUi) -> Needle {
    rp_ui.state().filter.lock().clone()
}

/// Update the cached filter needle. Folded on the way in, so the list and the
/// Most Played grid beside it share one needle.
///
/// Returns the token that names this needle. The Songs walk is bounded by the
/// 200-row recency set and stays on the UI thread; the Most Played walk is not,
/// so it is built on a worker and carries this back to prove it is still the
/// answer being asked for — see `RecentlyPlayedUi::filter_generation`.
pub fn set_filter(rp_ui: &RecentlyPlayedUi, filter: &str) -> u64 {
    *rp_ui.state().filter.lock() = row_match::fold_needle(filter);
    rp_ui.bump_filter_generation()
}

/// Fetch the 200 most-recently-played tracks, cache them, push the hero stats,
/// then apply the in-memory filter into the Slint model. Runs on a tokio worker;
/// the model write hops to the UI thread.
pub async fn refresh_tracks(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
) -> AppResult<()> {
    let rows = library::recently_played::get_recently_played(state).await?;

    // A leave that landed while the query was in flight has already cleared
    // `tracks_all` and emptied the model, so everything below would undo that
    // teardown behind a view nobody can see — the cover prewarm and the hero
    // recompose included. The leave set `mark_dirty`, so the next enter
    // re-fetches. Same guard, same placement, as `grid::fetch::refresh_grid`:
    // after the slow part, because before it the leave hasn't happened yet.
    if !rp_ui.section_active() {
        return Ok(());
    }

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

    // The decode burst above is an `.await`, so the guard has to be asked again:
    // a leave landing inside it has already emptied the models and wiped the
    // cache, and everything below would fill both back in behind a hidden view.
    // Same placement rule as the check before it — after the slow part, because
    // before it the leave hasn't happened yet.
    if !rp_ui.section_active() {
        return Ok(());
    }

    // Hero: count + total duration over the full (unfiltered) recency set, and
    // the up-to-4 most-recently-played distinct covers for the mosaic. All three
    // are folded here, on the worker that holds the rows — the band is per-tab
    // now, so a publish triggered from the *grid* has to be able to state them
    // too, which means they outlive this function and can't be arguments.
    let totals = SongsTotals {
        tracks: len_as_i32(rows.len()),
        duration_ms: rows.iter().map(|r| r.duration_ms).sum(),
    };
    let fold = crate::ui::hero_folds::fold_tracks(&rows);
    let mosaic_paths = super::hero::mosaic_paths_from(&rows, 4);

    // Under the section gate so the stores can't interleave with
    // `release_section_state`'s wipe and leave half of each on screen — the same
    // pairing `grid::fetch::refresh_grid` makes for `most_played`. Held across
    // the synchronous stores only, never across an `.await`, and taken *before*
    // the push below so the band can't publish a fold this hasn't landed yet.
    {
        let _gate = rp_ui.gate();
        *rp_ui.state().songs_totals.lock() = totals;
        *rp_ui.state().songs_fold.lock() = fold;
        *rp_ui.state().tracks_all.lock() = rows;
    }

    super::hero::push_hero_stats(totals.tracks, &mosaic_paths, rp_ui, weak);
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

    apply_filtered_tracks(rp_ui, weak);
    Ok(())
}

/// Re-walk the cached `tracks_all` through the active filter and push the
/// result into `RecentlyPlayed.tracks`, still in recency order. Cheap —
/// entirely in memory. Existing selection is re-stamped so a filter change
/// doesn't visually drop the user's selection.
///
/// A hidden section is never written to, the way
/// `grid::apply::write_filtered_grid` refuses to — and the check sits in
/// [`write_filtered_tracks`] rather than out here, because the leave can land
/// while the post is in flight.
///
/// **An unmounted tab is refused for the same reason, and asked about twice for
/// the reason every `section_active()` bail is.** `RecentlyPlayed.tracks` feeds
/// one element — `views/recently-played/songs-tab.slint`'s `TrackList`, under
/// `if tab-idx == tab-songs` — so on the Most Played tab every prepared row here
/// reaches nothing. [`build_filtered_tracks`]' check is what skips the cost, on
/// the two paths that reach here without going through a tab pick (the throttled
/// keystroke and the `library_changed` / `stats_changed` refresh, which are the
/// frequent ones); [`write_filtered_tracks`]' is what stops a pick landing
/// mid-post from leaving a row per track pinned behind a tab the user just left.
pub fn apply_filtered_tracks(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let Some(prepared) = build_filtered_tracks(rp_ui) else {
        return;
    };

    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        write_filtered_tracks(&ui, &rp_ui, prepared);
    });
}

/// Apply from the UI thread, with no event-loop hop — the rows land in the model
/// before Slint re-evaluates the `if` that mounts the entering tab.
///
/// The twin of `grid::apply::apply_filtered_grid_now`, and here for the same
/// reason: `slint::invoke_from_event_loop` posts even when it is called *from*
/// the UI thread, so a redraw can win the race. The tab-leave empties this model,
/// so what a lost race paints is a `TrackList` of headers over nothing.
pub fn apply_filtered_tracks_now(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    if let Some(prepared) = build_filtered_tracks(rp_ui) {
        write_filtered_tracks(ui, rp_ui, prepared);
    }
}

/// Walk the cached `tracks_all` through the active filter, or `None` when Songs
/// isn't the mounted tab and the walk would feed nothing.
///
/// Filters and prepares the `Send` row halves on the calling thread, borrowing
/// the cache in place — the `!Send` cover lookup is what `finish_track_list_row`
/// adds on the UI thread.
fn build_filtered_tracks(rp_ui: &RecentlyPlayedUi) -> Option<Vec<PreparedTrackRow>> {
    if rp_ui.active_tab() != RecentlyPlayedTab::Songs {
        return None;
    }
    let needle = current_filter(rp_ui);
    let all = rp_ui.state().tracks_all.lock();
    Some(
        all.iter()
            .filter(|r| row_match::track_matches(r, &needle))
            .map(crate::ui::tracks::prepare_track_list_row)
            .collect(),
    )
}

/// Finish the rows and push them into `RecentlyPlayed.tracks`. UI thread only.
///
/// Both gates are re-asked here rather than trusted from the build: on the
/// posting path a section leave or a tab pick can land while the closure is in
/// flight, and either one has already emptied this model on purpose.
fn write_filtered_tracks(
    ui: &AppWindow,
    rp_ui: &RecentlyPlayedUi,
    prepared: Vec<PreparedTrackRow>,
) {
    if !rp_ui.section_active() || rp_ui.active_tab() != RecentlyPlayedTab::Songs {
        return;
    }
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
