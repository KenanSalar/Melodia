//! Songs tab: fetch, sort persistence, in-memory filter, model apply.
//!
//! The Tracks view's fetch shape — one SQL fetch of the whole set per `library_changed_tx` tick,
//! with both the per-keystroke filter walk and the per-header-click re-sort running in memory off
//! [`crate::ui::track_list_cache`].

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::{FavoritesTab, FavoritesUi};
use crate::error::AppResult;
use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::row_match::{self, Needle};
use crate::ui::track_sort;
use crate::{AppWindow, Favorites, TrackListRow as UiTrackListRow};

/// The active sort. Slint mirrors it in `Favorites.sort-*`, but this cache is the source of truth
/// — those properties are written from Rust, never back.
pub fn current_sort(fav_ui: &FavoritesUi) -> ViewSort {
    fav_ui.state().sort.lock().clone()
}

/// Read-and-return the active filter needle, folded by [`set_filter`] and ready to hand to a
/// `row_match` predicate.
pub fn current_filter(fav_ui: &FavoritesUi) -> Needle {
    fav_ui.state().filter.lock().clone()
}

/// Update the cached sort. Callers follow it with [`resort_and_apply`] and a `set_view_sort`
/// persist, so the next launch lands on the same order.
pub fn set_sort(fav_ui: &FavoritesUi, field: String, dir: SortDir) {
    *fav_ui.state().sort.lock() = ViewSort { field, dir };
}

/// `"asc"` / `"desc"` for the shared comparator.
fn dir_token(dir: SortDir) -> &'static str {
    match dir {
        SortDir::Asc => "asc",
        SortDir::Desc => "desc",
    }
}

/// Whether a header click moved the sort between two reads of the shadow. Compares the direction
/// through [`dir_token`] because `ViewSort` carries no `PartialEq`.
fn sort_changed(a: &ViewSort, b: &ViewSort) -> bool {
    a.field != b.field || dir_token(a.dir) != dir_token(b.dir)
}

/// Re-sort the Songs tab after a header click, entirely in memory: the set is already resident and
/// already covered, and only the display permutation moves, so re-issuing the query would be an
/// unbounded `SELECT` plus a full cover prewarm for nothing.
pub fn resort_and_apply(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let sort = current_sort(fav_ui);
    fav_ui.state().tracks_all.resort(&sort.field, dir_token(sort.dir));
    apply_filtered_tracks(fav_ui, weak);
}

/// Update the cached filter needle, so the live-refresh subscriber can re-walk on a background
/// thread without reading the Slint global. Folded on the way in, so all four readers share one.
pub fn set_filter(fav_ui: &FavoritesUi, filter: &str) {
    *fav_ui.state().filter.lock() = row_match::fold_needle(filter);
}

/// Fetch the full favourites list, cache it under the current sort's display order, then re-apply
/// the filter. Runs on a tokio worker; the sort is resolved here rather than in SQL — see
/// [`resort_and_apply`].
pub async fn refresh_tracks(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
) -> AppResult<()> {
    let sort = current_sort(fav_ui);

    // Fetched in the query's fixed `sort_key` order; display order is derived in memory below, so
    // a cold fetch and a header click share one code path.
    //
    // Both ways of storing nothing re-arm the flag, the tab pick having *consumed* it before
    // spawning this: without the re-arm a failed query leaves the sentinel with no answer coming,
    // and the pick that would have re-asked believes the cache is current.
    let rows = library::favorites::get_favorite_tracks(state)
        .await
        .inspect_err(|_| fav_ui.mark_songs_dirty())?;

    // A leave landing while the query was in flight has already wiped `tracks_all` and emptied the
    // model, so everything below would undo that teardown behind a view nobody can see.
    if !fav_ui.section_active() {
        fav_ui.mark_songs_dirty();
        return Ok(());
    }

    // Warm the row covers before the first apply, or a cold cache pays one synchronous decode per
    // unique cover at paint time. Walked through the display permutation so the prefix surviving
    // the cap is what paints first.
    //
    // The permutation is computed here rather than inside `store_in_order` because it is needed
    // *now*, ahead of the guard that decides whether the store may happen at all.
    let order = track_sort::compute_track_order(&rows, &sort.field, dir_token(sort.dir));
    let cover_paths: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        order.iter().map(|&i| rows[i].artwork_path.as_deref()),
        fav_ui.cover_thumbs.capacity(),
    );
    if !cover_paths.is_empty() {
        let thumbs = fav_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&cover_paths)).await;
    }

    // The decode burst is an `.await`, so the guard is asked again — after the slow part, because
    // before it the leave hasn't happened yet.
    if !fav_ui.section_active() {
        fav_ui.mark_songs_dirty();
        return Ok(());
    }

    // The Songs tab's chips, folded on the worker that already holds the rows. Outside the gate:
    // what must not straddle `release_section_state`'s wipe is the pair of *stores*, so the walk
    // has no business holding a lock the wipe and both sibling fetches queue behind.
    let fold = crate::ui::hero_folds::fold_tracks(&rows);

    // A header click landing mid-fetch re-sorts the cache it finds and returns, with no second
    // query behind it to correct the order — so this store would overwrite it with the permutation
    // computed before the click. Asked twice because `store_in_order` converts every favourite: a
    // click landing inside it leaves the header naming one order and the list showing another.
    let sort_used = current_sort(fav_ui);
    let order = if sort_changed(&sort_used, &sort) {
        track_sort::compute_track_order(&rows, &sort_used.field, dir_token(sort_used.dir))
    } else {
        order
    };

    {
        let _gate = fav_ui.gate();
        *fav_ui.state().songs_fold.lock() = fold;
        fav_ui.state().tracks_all.store_in_order(rows, order);
    }
    let sort_now = current_sort(fav_ui);
    if sort_changed(&sort_now, &sort_used) {
        fav_ui.state().tracks_all.resort(&sort_now.field, dir_token(sort_now.dir));
    }
    // `kick_full_refresh` runs this concurrently with the hero and grid fetches, so whichever of
    // those published did it against the previous fold.
    super::hero::republish_chips(fav_ui, weak);

    apply_filtered_tracks(fav_ui, weak);
    Ok(())
}

/// Re-walk the cached `tracks_all` through the active filter and push the result into
/// `Favorites.tracks`. Selected ids are re-stamped before the swap, so a filter change or library
/// refresh doesn't visually drop a selection.
///
/// A hidden section is never written to, and the check must sit in [`write_filtered_tracks`]
/// rather than out here because the leave can land while the post is in flight.
///
/// **An unmounted tab is refused for the same reason, and asked about twice.** `Favorites.tracks`
/// feeds one element under `if tab-idx == tab-songs`, so on the other two tabs every row built
/// here reaches nothing — [`build_filtered_tracks`]' check skips the cost, and
/// [`write_filtered_tracks`]' stops a pick landing mid-post from pinning a row per favourite
/// behind a tab the user just left.
pub fn apply_filtered_tracks(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let Some(rows) = build_filtered_tracks(fav_ui) else {
        return;
    };

    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        write_filtered_tracks(&ui, &fav_ui, rows);
    });
}

/// Apply from the UI thread, with no event-loop hop — the rows land in the model before Slint
/// re-evaluates the `if` that mounts the entering tab.
///
/// `grids::apply::apply_filtered_grids_now`'s twin: `invoke_from_event_loop` posts even when
/// called *from* the UI thread, so a redraw can win the race — and the tab-leave emptied this
/// model, so what a lost race paints is a `TrackList` of headers over nothing.
pub fn apply_filtered_tracks_now(ui: &AppWindow, fav_ui: &FavoritesUi) {
    if let Some(rows) = build_filtered_tracks(fav_ui) {
        write_filtered_tracks(ui, fav_ui, rows);
    }
}

/// Walk the cached `tracks_all` through the active filter, or `None` when Songs isn't the mounted
/// tab and the walk would feed nothing.
///
/// Runs on the calling thread off one `Arc` snapshot and allocates nothing per row — the cache
/// holds converted rows, so a survivor is cloned, not rebuilt.
fn build_filtered_tracks(fav_ui: &FavoritesUi) -> Option<Vec<UiTrackListRow>> {
    if fav_ui.active_tab() != FavoritesTab::Songs {
        return None;
    }
    let needle = current_filter(fav_ui);
    Some(fav_ui.state().tracks_all.snapshot().visible(&needle))
}

/// Push the rows into `Favorites.tracks`. UI thread only.
///
/// Both gates are re-asked here rather than trusted from the build: on the posting path a section
/// leave or a tab pick can land while the closure is in flight, and either has already emptied
/// this model on purpose.
fn write_filtered_tracks(ui: &AppWindow, fav_ui: &FavoritesUi, mut rendered: Vec<UiTrackListRow>) {
    if !fav_ui.section_active() || fav_ui.active_tab() != FavoritesTab::Songs {
        return;
    }
    let g = ui.global::<Favorites>();
    let model = g.get_tracks();
    let Some(vec) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        log::warn!("Favorites.tracks: VecModel<TrackListRow> downcast failed");
        return;
    };
    super::selection::restamp_rows(&g, &mut rendered);
    // A per-row rewrite where the identities align — a library tick — keeps the `ListView`'s
    // delegates instead of tearing them all down; a structural change falls back to `set_vec`.
    crate::ui::model_diff::apply_rows_keyed(vec, rendered, |r| r.id);
}

/// Set `rating` on one Songs-tab row. Rating affects neither membership nor
/// order, so the row stays put and is patched in place rather than the whole
/// filtered list rebuilt. Mirrors [`crate::ui::tracks::apply_row_rating`].
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::model_patch::patch_track_row_by_id(
            &ui.global::<Favorites>().get_tracks(),
            id,
            |r| r.rating = rating,
        );
    });
}
