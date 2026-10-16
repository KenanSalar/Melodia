//! Songs tab: fetch, sort persistence, in-memory filter, model apply.
//!
//! The Tracks view's fetch shape — one SQL fetch of the whole set per `library_changed` tick,
//! with both the per-keystroke filter walk and the per-header-click re-sort running in memory off
//! [`crate::ui::track_list_cache`].

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::{FavoritesTab, FavoritesUi};
use crate::ui::row_match::{self, Needle};
use crate::ui::track_list_cache::VisibleRows;
use crate::ui::track_list_model::TrackListModel;
use crate::ui::track_sort;
use melodia_app::library;
use melodia_app::services::settings::{SortDir, ViewSort};
use melodia_app::state::AppState;
use melodia_core::error::AppResult;
use melodia_ui::{AppWindow, Favorites};

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
/// the filter. Runs on whichever thread awaits it: a tokio worker from the section enter, the UI
/// thread from the live-refresh subscriber. The sort is resolved here rather than in SQL; see
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
    let rows = library::favorites::get_favorite_tracks(&state.db)
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

/// Re-walk the cached `tracks_all` through the active filter and show the result in
/// `Favorites.tracks`. The model keeps its own selection, so a filter change or library refresh
/// doesn't visually drop one.
///
/// A hidden section is never written to, and the check must sit in [`write_filtered_tracks`]
/// rather than out here because the leave can land while the post is in flight.
///
/// **An unmounted tab is refused for the same reason, and asked about twice.** `Favorites.tracks`
/// feeds one element under `if tab-idx == tab-songs`, so on the other two tabs the walk here
/// reaches nothing — [`build_filtered_tracks`]' check skips the cost, and
/// [`write_filtered_tracks`]' stops a pick landing mid-post from pinning the whole favourites
/// snapshot behind a tab the user just left.
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
/// Runs on the calling thread off one `Arc` snapshot and copies no row: the model reads the
/// cache's own.
fn build_filtered_tracks(fav_ui: &FavoritesUi) -> Option<VisibleRows> {
    if fav_ui.active_tab() != FavoritesTab::Songs {
        return None;
    }
    let needle = current_filter(fav_ui);
    Some(VisibleRows::new(fav_ui.state().tracks_all.snapshot(), &needle))
}

/// Show the rows in `Favorites.tracks`. UI thread only.
///
/// Both gates are re-asked here rather than trusted from the build: on the posting path a section
/// leave or a tab pick can land while the closure is in flight, and either has already emptied
/// this model on purpose.
fn write_filtered_tracks(ui: &AppWindow, fav_ui: &FavoritesUi, visible: VisibleRows) {
    if !fav_ui.section_active() || fav_ui.active_tab() != FavoritesTab::Songs {
        return;
    }
    let rows = ui.global::<Favorites>().get_tracks();
    if let Some(model) = TrackListModel::installed(&rows, "Favorites.tracks") {
        model.show(visible);
    }
}

/// Repaint one Songs-tab row after a rating patch to the cache. Rating affects neither
/// membership nor order, so the row stays put rather than the whole filtered list being rebuilt.
/// Mirrors [`crate::ui::tracks::repaint_row`].
pub fn repaint_row(weak: &Weak<AppWindow>, id: i64) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let rows = ui.global::<Favorites>().get_tracks();
        if let Some(model) = TrackListModel::installed(&rows, "Favorites.tracks") {
            model.repaint(id);
        }
    });
}

/// Empty `Favorites.tracks`, handing back the snapshot it shows. The selection stays, as
/// `selected-ids` does, so a tab leave paints it again on return; a section leave clears it first.
pub fn hide_tracks(g: &Favorites) {
    let rows = g.get_tracks();
    if let Some(model) = TrackListModel::installed(&rows, "Favorites.tracks") {
        model.show(VisibleRows::default());
    }
}
