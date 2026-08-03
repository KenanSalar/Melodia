//! Songs tab: fetch, sort persistence, in-memory filter, model
//! apply. Mirrors the Tracks view's fetch shape — the SQL fetch returns
//! the entire sorted set once per `library_changed_tx` tick, and the
//! per-keystroke filter walk is in memory over the fields
//! [`crate::ui::row_match`] defines.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::FavoritesUi;
use crate::error::AppResult;
use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::row_match;
use crate::ui::tracks::{PreparedTrackRow, finish_track_list_row};
use crate::{AppWindow, Favorites, TrackListRow as UiTrackListRow};

/// Read-and-return the active sort. The Slint side mirrors this in
/// `Favorites.sort-field` / `Favorites.sort-dir`, but the Rust cache
/// is the source of truth (Slint properties are written from Rust,
/// not the reverse — `set-sort-*` callbacks update the cache + persist
/// + re-fetch).
pub fn current_sort(fav_ui: &FavoritesUi) -> ViewSort {
    fav_ui.state().sort.lock().clone()
}

/// Read-and-return the active filter needle, already folded by
/// [`set_filter`] and ready to hand to a `row_match` predicate.
pub fn current_filter(fav_ui: &FavoritesUi) -> String {
    fav_ui.state().filter.lock().clone()
}

/// Update the cached sort. Callers (`wire_favorites`) are expected to
/// follow this with a `refresh_tracks` call + a `set_view_sort` persist
/// so the next launch lands on the same order.
pub fn set_sort(fav_ui: &FavoritesUi, field: String, dir: SortDir) {
    *fav_ui.state().sort.lock() = ViewSort { field, dir };
}

/// Update the cached filter needle. The Slint side already holds the
/// live text via `<=>` binding; this mirror lets the live-refresh
/// subscriber re-walk on a background thread without re-reading the
/// Slint global. Folded on the way in, so all four readers — the Songs
/// list, both grids, and the two queue-id walks — share one needle.
pub fn set_filter(fav_ui: &FavoritesUi, filter: &str) {
    *fav_ui.state().filter.lock() = row_match::fold_needle(filter);
}

/// Fetch the full favourites list at the current sort, cache it in
/// Rust, then re-apply the in-memory filter so the Slint model
/// reflects the new data. Runs on a tokio worker; the model write
/// happens via `upgrade_in_event_loop` because Slint models are UI-
/// thread only.
pub async fn refresh_tracks(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
) -> AppResult<()> {
    let sort = current_sort(fav_ui);
    let sort_by = Some(sort.field.clone());
    let sort_dir = Some(match sort.dir {
        SortDir::Asc => "asc".to_owned(),
        SortDir::Desc => "desc".to_owned(),
    });

    let rows = library::favorites::get_favorite_tracks(state, sort_by, sort_dir).await?;

    // Prewarm the row covers off-thread before the first model apply:
    // the `!Send` cover lookup in `finish_track_list_row` runs on the UI
    // thread, so a cold cache (first section enter) would otherwise pay
    // one synchronous decode per unique favourite cover at paint time.
    // The rows arrive in the sort's display order, so the prefix surviving
    // the cap is the one that paints first — pre-filter, which only diverges
    // on a library refresh taken with a filter already narrowing the set past
    // the cap. Keystroke re-filters skip this entirely: they only narrow an
    // already-painted (warm) set.
    let cover_paths: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        rows.iter().map(|r| r.artwork_path.as_deref()),
        fav_ui.cover_thumbs.capacity(),
    );
    if !cover_paths.is_empty() {
        let thumbs = fav_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&cover_paths)).await;
    }

    *fav_ui.state().tracks_all.lock() = rows;
    // The Songs tab's artist / album chips are folded out of the cache that
    // line just filled, and nothing else republishes after it — `kick_full_refresh`
    // runs this task concurrently with the hero and grid fetches, so whichever
    // of those published did so against an empty `tracks_all`. Here, not in
    // `apply_filtered_tracks`: that also runs per keystroke, and the fold walks
    // every favourite.
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
pub fn apply_filtered_tracks(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let needle = current_filter(fav_ui);

    // Filter + prepare the `Send` row halves on the calling thread,
    // borrowing the cache in place — the old path deep-cloned the whole
    // String-bearing Vec per keystroke and then built every UI row
    // (including the `!Send` cover lookup) inside the event-loop closure.
    let prepared: Vec<PreparedTrackRow> = {
        let all = fav_ui.state().tracks_all.lock();
        all.iter()
            .filter(|r| row_match::track_matches(r, &needle))
            .map(crate::ui::tracks::prepare_track_list_row)
            .collect()
    };
    let filtered_count = i32::try_from(prepared.len()).unwrap_or(i32::MAX);

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let model = g.get_tracks();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
            log::warn!("Favorites.tracks: VecModel<TrackListRow> downcast failed");
            return;
        };
        let mut rendered: Vec<UiTrackListRow> = prepared
            .into_iter()
            .map(finish_track_list_row)
            .collect();
        super::selection::restamp_rows(&g, &mut rendered);
        // Per-row rewrite when identities align (same-shape refresh, e.g.
        // a library tick): keeps the ListView's delegates instead of the
        // tear-down-everything `set_vec` reset. Structural changes
        // (filter narrowed/widened) fall back to `set_vec`.
        crate::ui::model_diff::apply_rows_keyed(vec, rendered, |r| r.id);
        g.set_filtered_count(filtered_count);
    });
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
