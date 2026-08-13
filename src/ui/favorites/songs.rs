//! Songs tab: fetch, sort persistence, in-memory filter, model
//! apply. Mirrors the Tracks view's fetch shape — the SQL fetch returns
//! the entire set once per `library_changed_tx` tick, and both the
//! per-keystroke filter walk and the per-header-click re-sort run in memory
//! off [`crate::ui::track_list_cache`].

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

/// Read-and-return the active sort. The Slint side mirrors this in
/// `Favorites.sort-field` / `Favorites.sort-dir`, but the Rust cache
/// is the source of truth (Slint properties are written from Rust,
/// not the reverse — `set-sort-*` callbacks update the cache + persist
/// + re-fetch).
pub fn current_sort(fav_ui: &FavoritesUi) -> ViewSort {
    fav_ui.state().sort.lock().clone()
}

/// Read-and-return the active filter needle, folded by [`set_filter`] and ready
/// to hand to a `row_match` predicate.
pub fn current_filter(fav_ui: &FavoritesUi) -> Needle {
    fav_ui.state().filter.lock().clone()
}

/// Update the cached sort. Callers (`callbacks::tracklist`) are expected to
/// follow this with a [`resort_and_apply`] call + a `set_view_sort` persist
/// so the next launch lands on the same order.
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

/// Whether a header click moved the sort between two reads of the shadow.
/// Compares the direction through [`dir_token`] because `ViewSort` carries no
/// `PartialEq` and the token is what the comparator is handed anyway.
fn sort_changed(a: &ViewSort, b: &ViewSort) -> bool {
    a.field != b.field || dir_token(a.dir) != dir_token(b.dir)
}

/// Re-sort the Songs tab after a header-column click, entirely in memory.
///
/// This used to re-issue `get_favorite_tracks` with a new `ORDER BY` — an
/// unbounded `SELECT` plus a full cover prewarm per click, for a set already
/// resident and already covered. Only the display permutation changes, so
/// only the display permutation is recomputed; the Tracks view has resolved
/// its header clicks this way all along.
pub fn resort_and_apply(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let sort = current_sort(fav_ui);
    fav_ui.state().tracks_all.resort(&sort.field, dir_token(sort.dir));
    apply_filtered_tracks(fav_ui, weak);
}

/// Update the cached filter needle. The Slint side already holds the
/// live text via `<=>` binding; this mirror lets the live-refresh
/// subscriber re-walk on a background thread without re-reading the
/// Slint global. Folded on the way in, so all four readers — the Songs
/// list, both grids, and the two queue-id walks — share one needle.
pub fn set_filter(fav_ui: &FavoritesUi, filter: &str) {
    *fav_ui.state().filter.lock() = row_match::fold_needle(filter);
}

/// Fetch the full favourites list, cache it in Rust under the current
/// sort's display order, then re-apply the in-memory filter so the Slint
/// model reflects the new data. Runs on a tokio worker; the model write
/// happens via `upgrade_in_event_loop` because Slint models are UI-
/// thread only.
///
/// The sort is resolved here rather than in SQL — see [`resort_and_apply`],
/// which is what a header click takes instead of a second query.
pub async fn refresh_tracks(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
) -> AppResult<()> {
    let sort = current_sort(fav_ui);

    // Fetched in the query's fixed `sort_key` order — display order is derived
    // in memory below, so the cold fetch and a later header click share the
    // one `compute_track_order` code path.
    // Both ways of storing nothing re-arm the flag, because the tab pick
    // *consumes* it before spawning this: without the re-arm a failed query
    // leaves the sentinel with no answer coming, and the pick that would have
    // re-asked believes the cache is current.
    let rows = library::favorites::get_favorite_tracks(state)
        .await
        .inspect_err(|_| fav_ui.mark_songs_dirty())?;

    // A leave that landed while the query was in flight has already wiped
    // `tracks_all` and emptied the model, so everything below would undo that
    // teardown behind a view nobody can see. Nothing is lost by dropping the
    // result: every leave sets `mark_dirty`, so the next enter re-fetches. Same
    // guard, same placement, as `grids::fetch::refresh_grids`.
    if !fav_ui.section_active() {
        fav_ui.mark_songs_dirty();
        return Ok(());
    }

    // Prewarm the row covers off-thread before the first model apply: a cold
    // cache (first section enter) would otherwise pay one synchronous decode
    // per unique favourite cover at paint time. Walked through the display
    // permutation so the prefix surviving the cap is the one that paints
    // first — pre-filter, which only diverges on a library refresh taken with
    // a filter already narrowing the set past the cap. Keystroke re-filters
    // skip this entirely: they only narrow an already-painted (warm) set.
    //
    // The permutation is computed here rather than inside `store_in_order`
    // because it is needed *now*, ahead of the guard that decides whether the
    // store may happen at all.
    let order = track_sort::compute_track_order(&rows, &sort.field, dir_token(sort.dir));
    let cover_paths: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        order.iter().map(|&i| rows[i].artwork_path.as_deref()),
        fav_ui.cover_thumbs.capacity(),
    );
    if !cover_paths.is_empty() {
        let thumbs = fav_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&cover_paths)).await;
    }

    // The decode burst above is an `.await`, so the guard has to be asked
    // again — after the slow part, because before it the leave hasn't happened
    // yet.
    if !fav_ui.section_active() {
        fav_ui.mark_songs_dirty();
        return Ok(());
    }

    // The Songs tab's artist / album chips, folded here — on the worker that
    // holds the rows, before they go into the cache, which is the whole of why
    // `publish_favorites` costs nothing. Outside the gate: what must not land
    // either side of `release_section_state`'s wipe is the pair of *stores*, so
    // the walk that produces the fold has no business holding a lock the wipe
    // and both sibling fetches queue behind.
    let fold = crate::ui::hero_folds::fold_tracks(&rows);

    // A header click can land while this fetch is in flight. It re-sorts the
    // cache it finds and returns, so unlike the re-fetch it replaced there is
    // no second query to correct the order afterwards — this store would just
    // overwrite it with the permutation computed before the click. Asked twice
    // for the reason every guard around a slow step is: `store_in_order`
    // converts every favourite, so a click landing inside it would otherwise
    // leave the header naming one order and the list showing another until the
    // next library tick. Both recomputes only run on the race.
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
    // Then republish: `kick_full_refresh` runs this task concurrently with the
    // hero and grid fetches, so whichever of those published did so against the
    // previous fold.
    super::hero::republish_chips(fav_ui, weak);

    apply_filtered_tracks(fav_ui, weak);
    Ok(())
}

/// Re-walk the cached `tracks_all` through the active filter and push
/// the result into `Favorites.tracks`. Cheap — runs entirely in memory.
/// The filter match is case-insensitive across title + artist + album,
/// same shape as the Tracks view's `apply_filter`. Any rows whose id is
/// currently in `Favorites.selected-ids` get their `selected` flag
/// re-stamped before the swap, so a filter change / library refresh
/// doesn't visually drop an existing selection (the row may shift index
/// but its checkbox + accent background hold).
///
/// A hidden section is never written to, the way
/// `grids::apply::write_filtered_grids` refuses to: the leave teardown empties
/// this model deliberately, and the check has to sit in [`write_filtered_tracks`]
/// rather than out here, because the leave can land while the post is in flight.
///
/// **An unmounted tab is refused for the same reason, and asked about twice for
/// the reason every `section_active()` bail is.** `Favorites.tracks` feeds one
/// element — `views/favorites/songs-tab.slint`'s `TrackList`, under
/// `if tab-idx == tab-songs` — so on the other two tabs every row built here
/// reaches nothing. The gate is not new: `callbacks::favorites::subviews`'
/// tab-change handler already spells it out for its own call, having priced one
/// row per favourite on this thread. What it couldn't cover is the two
/// paths that reach here without going through a tab pick — the throttled
/// keystroke and the `library_changed` / `stats_changed` refresh — and those are
/// the frequent ones. [`build_filtered_tracks`]' check is what skips the cost;
/// [`write_filtered_tracks`]' is what stops a pick landing mid-post from leaving
/// a row per favourite pinned behind a tab the user just left.
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

/// Apply from the UI thread, with no event-loop hop — the rows land in the model
/// before Slint re-evaluates the `if` that mounts the entering tab.
///
/// The twin of `grids::apply::apply_filtered_grids_now`, and here for the same
/// reason: `slint::invoke_from_event_loop` posts even when it is called *from*
/// the UI thread, so a redraw can win the race. The tab-leave empties this model,
/// so what a lost race paints is a `TrackList` of headers over nothing.
pub fn apply_filtered_tracks_now(ui: &AppWindow, fav_ui: &FavoritesUi) {
    if let Some(rows) = build_filtered_tracks(fav_ui) {
        write_filtered_tracks(ui, fav_ui, rows);
    }
}

/// Walk the cached `tracks_all` through the active filter, or `None` when Songs
/// isn't the mounted tab and the walk would feed nothing.
///
/// Runs on the calling thread off one `Arc` snapshot, and allocates nothing
/// per row: the cache holds converted rows, so a surviving row is cloned
/// rather than rebuilt. The old path deep-cloned the whole String-bearing Vec
/// per keystroke and then built every UI row inside the event-loop closure.
fn build_filtered_tracks(fav_ui: &FavoritesUi) -> Option<Vec<UiTrackListRow>> {
    if fav_ui.active_tab() != FavoritesTab::Songs {
        return None;
    }
    let needle = current_filter(fav_ui);
    Some(fav_ui.state().tracks_all.snapshot().visible(&needle))
}

/// Push the rows into `Favorites.tracks`. UI thread only.
///
/// Both gates are re-asked here rather than trusted from the build: on the
/// posting path a section leave or a tab pick can land while the closure is in
/// flight, and either one has already emptied this model on purpose.
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
    // Per-row rewrite when identities align (same-shape refresh, e.g. a library
    // tick): keeps the ListView's delegates instead of the tear-down-everything
    // `set_vec` reset. Structural changes (filter narrowed/widened) fall back to
    // `set_vec`.
    crate::ui::model_diff::apply_rows_keyed(vec, rendered, |r| r.id);
}

/// Set `rating` on a single Songs-tab row in the Slint `VecModel`. Rating is
/// independent of favorite membership (and there is no in-table rating sort),
/// so the row stays put — patch it in place rather than rebuilding the whole
/// filtered list. Mirrors [`crate::ui::tracks::apply_row_rating`].
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::model_patch::patch_track_row_by_id(
            &ui.global::<Favorites>().get_tracks(),
            id,
            |r| r.rating = rating,
        );
    });
}
