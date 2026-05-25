//! All Songs list: fetch, sort persistence, in-memory filter, model
//! apply. Mirrors the Tracks view's fetch shape — the SQL fetch returns
//! the entire sorted set once per `library_changed_tx` tick, and the
//! per-keystroke filter walk is in memory (matches title + artist +
//! album case-insensitive).

use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::FavoritesUi;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::tracks::to_slint_track_list_row;
use crate::{AppWindow, Favorites, TrackListRow as UiTrackListRow};

/// Read-and-return the active sort. The Slint side mirrors this in
/// `Favorites.sort-field` / `Favorites.sort-dir`, but the Rust cache
/// is the source of truth (Slint properties are written from Rust,
/// not the reverse — `set-sort-*` callbacks update the cache + persist
/// + re-fetch).
pub fn current_sort(fav_ui: &FavoritesUi) -> ViewSort {
    fav_ui.state().sort.lock().clone()
}

/// Read-and-return the active filter string.
pub fn current_filter(fav_ui: &FavoritesUi) -> String {
    fav_ui.state().filter.lock().clone()
}

/// Update the cached sort. Callers (`wire_favorites`) are expected to
/// follow this with a `refresh_tracks` call + a `set_view_sort` persist
/// so the next launch lands on the same order.
pub fn set_sort(fav_ui: &FavoritesUi, field: String, dir: SortDir) {
    *fav_ui.state().sort.lock() = ViewSort { field, dir };
}

/// Update the cached filter string. The Slint side already holds the
/// live text via `<=>` binding; this mirror lets the live-refresh
/// subscriber re-walk on a background thread without re-reading the
/// Slint global.
pub fn set_filter(fav_ui: &FavoritesUi, filter: String) {
    *fav_ui.state().filter.lock() = filter;
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
    *fav_ui.state().tracks_all.lock() = rows;

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
    let needle = current_filter(fav_ui).to_lowercase();
    let all = fav_ui.state().tracks_all.lock().clone();
    let thumbs = fav_ui.cover_thumbs.clone();

    let filtered: Vec<RsTrackListRow> = if needle.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|r| crate::ui::detail_filter::track_matches(r, &needle))
            .collect()
    };
    let filtered_count = i32::try_from(filtered.len()).unwrap_or(i32::MAX);

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let model = g.get_tracks();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
            log::warn!("Favorites.tracks: VecModel<TrackListRow> downcast failed");
            return;
        };
        let mut rendered: Vec<UiTrackListRow> = filtered
            .iter()
            .map(|t| to_slint_track_list_row(t, &thumbs))
            .collect();
        super::selection::restamp_rows(&g, &mut rendered);
        vec.set_vec(rendered);
        g.set_filtered_count(filtered_count);
    });
}
