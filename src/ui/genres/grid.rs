//! Genres grid: DB fetch + filter / sort / chunk logic.
//!
//! Mirror of `src/ui/albums/grid.rs` minus everything cover-related: no
//! prewarm step, no cap-tuning function, no `first_screenful_paths`.
//! Genres have no artwork (see the `Genres` global comment in
//! `melodia-ui/ui/globals.slint`).

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::state::{GridData, GridIndexCache};
use super::{GenresUi, to_slint_genre_row};
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::{
    AppWindow, GenreGridRow as UiGenreGridRow, GenreRow as UiGenreRow, Genres,
};

/// Fetch the genre list from the DB into `genres_ui.grid.data`, then
/// rebuild the grid model on the UI thread. Async — runs on the tokio
/// runtime; the UI write hops back via `upgrade_in_event_loop`. Called
/// once at startup and from the library-changed subscriber. The
/// pre-lowercased search keys are built here (on the worker), not per
/// keystroke on the UI thread.
///
/// No prewarm step: genres have no artwork to decode (compare
/// `src/ui/albums/grid.rs::fetch_grid`, which prewarms the first screenful
/// of grid-tier covers).
pub async fn fetch_grid(
    state: &AppState,
    genres_ui: &Arc<GenresUi>,
    weak: Weak<AppWindow>,
) -> AppResult<()> {
    let genres = library::genres::get_genres(state).await?;
    let data = Arc::new(GridData::new(genres));
    // See `ui::albums::grid::fetch_grid` for the gate rationale.
    {
        let _gate = genres_ui.section.gate();
        *genres_ui.grid.data.lock() = data;
        // The genre set changed — the memoized filter+sort indices are stale.
        *genres_ui.grid.index_cache.lock() = None;
    }

    let genres_ui = genres_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        rebuild_grid(&ui, &genres_ui);
    });
    Ok(())
}

/// Rebuild the grid model from the cached grid data — no DB hit. Runs
/// on the UI thread (called directly from the `apply-filter` /
/// `request-sort` / `columns-changed` callbacks, which have already
/// updated the `Genres` global, and from `fetch_grid`'s UI hop).
///
/// The filter+sort result is memoized in `grid.index_cache`: a pure
/// `columns-changed` (the common case while resizing the window or
/// toggling the sidebar) reuses the cached indices and only re-chunks,
/// skipping the filter walk and the sort entirely.
pub fn rebuild_grid(ui: &AppWindow, genres_ui: &GenresUi) {
    let g = ui.global::<Genres>();
    let sort_field = g.get_sort_field().to_string();
    let sort_dir = g.get_sort_dir().to_string();
    let filter = g.get_filter().to_string();
    let columns = g.get_columns().max(1);

    let data = genres_ui.grid.data.lock().clone();

    let rows = {
        let mut cache = genres_ui.grid.index_cache.lock();
        let stale =
            !matches!(cache.as_ref(), Some(c) if c.matches(&filter, &sort_field, &sort_dir));
        if stale {
            let indices = compute_indices(&data, &sort_field, &sort_dir, &filter);
            *cache = Some(GridIndexCache {
                filter,
                sort_field,
                sort_dir,
                indices,
            });
        }
        // `cache` is now `Some` either way — a stale entry was just
        // recomputed, a fresh one was left in place.
        let indices = cache.as_ref().map_or(&[][..], |c| c.indices.as_slice());
        chunk_indices(&data, indices, columns)
    };
    let total = i32::try_from(data.genres.len()).unwrap_or(i32::MAX);

    g.set_total_count(total);
    let model = g.get_grid_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiGenreGridRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_grid_rows(ModelRc::new(VecModel::from(rows)));
    }
}

/// Filter + sort the grid data into a display-order list of genre
/// indices. Pure / no UI state. The filter walk and the name sort read
/// `data.keys` (pre-lowercased in `fetch_grid`), so this allocates
/// nothing per genre beyond the index `Vec` itself.
pub(super) fn compute_indices(
    data: &GridData,
    sort_field: &str,
    sort_dir: &str,
    filter: &str,
) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    let mut indices: Vec<usize> = if needle.is_empty() {
        (0..data.genres.len()).collect()
    } else {
        data.keys
            .iter()
            .enumerate()
            .filter(|(_, k)| k.name_lc.contains(&needle))
            .map(|(i, _)| i)
            .collect()
    };
    sort_genre_indices(&mut indices, data, sort_field, sort_dir);
    indices
}

/// Chunk a display-order index list into rows of `columns` `GenreRow`
/// cards. Pure; this is the only step a `columns-changed` rebuild has
/// to redo (the filter+sort `indices` are reused from
/// `grid.index_cache`).
fn chunk_indices(data: &GridData, indices: &[usize], columns: i32) -> Vec<UiGenreGridRow> {
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut rows: Vec<UiGenreGridRow> = Vec::with_capacity(indices.len().div_ceil(cols));
    for chunk in indices.chunks(cols) {
        let cards: Vec<UiGenreRow> = chunk
            .iter()
            .map(|&i| to_slint_genre_row(&data.genres[i]))
            .collect();
        rows.push(UiGenreGridRow {
            genres: ModelRc::from(Rc::new(VecModel::from(cards))),
        });
    }
    rows
}

/// Sort `indices` into the grid data by the chosen field. `genre_stats`
/// is fetched name-ASC, so the Track-count / Duration sorts (and `desc`
/// on any field) must be done in memory — the DB query order is fixed.
/// Reads the pre-lowercased `data.keys`, so `sort_by_cached_key` caches
/// `&str` references rather than re-allocating a lowercased `String` per
/// genre. Each sort uses `name_lc` as the deterministic tie-breaker.
fn sort_genre_indices(indices: &mut [usize], data: &GridData, field: &str, dir: &str) {
    match field {
        "track_count" => indices.sort_by_cached_key(|&i| {
            (data.genres[i].track_count, data.keys[i].name_lc.as_str())
        }),
        "duration" => indices.sort_by_cached_key(|&i| {
            (data.genres[i].total_duration_ms, data.keys[i].name_lc.as_str())
        }),
        _ => indices.sort_by_cached_key(|&i| data.keys[i].name_lc.as_str()),
    }
    if dir == "desc" {
        indices.reverse();
    }
}
