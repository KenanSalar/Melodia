//! Albums grid: DB fetch + filter / sort / chunk / prewarm logic, plus the
//! display-aware cover-cache cap tuner.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::state::{
    DEFAULT_GRID_COVER_CAP, GRID_PREWARM_AHEAD, GridData, GridIndexCache,
};
use super::{AlbumsUi, to_slint_album_row};
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::{
    AlbumGridRow as UiAlbumGridRow, AlbumRow as UiAlbumRow, Albums, AppWindow,
};

/// Fetch the album list from the DB into `albums_ui.grid.data`, prewarm
/// cover thumbnails, then rebuild the grid model on the UI thread. Async —
/// runs on the tokio runtime; the UI write hops back via
/// `upgrade_in_event_loop`. Called once at startup and from the
/// library-changed subscriber. The pre-lowercased search / sort keys are
/// built here (on the worker), not per keystroke on the UI thread.
pub async fn fetch_grid(
    state: &AppState,
    albums_ui: &Arc<AlbumsUi>,
    weak: Weak<AppWindow>,
) -> AppResult<()> {
    let albums = library::albums::get_albums(state).await?;
    let data = Arc::new(GridData::new(albums));
    // Serialize against `AlbumsUi::release_section_state`'s wipe via the
    // shared section gate. Without this serialization, a fast leave→
    // re-enter could let the wipe land *between* this fresh-data write and
    // the upcoming UI repaint, painting an empty grid. The gate is held
    // only across the synchronous writes — never across an `.await`.
    {
        let _gate = albums_ui.section.gate();
        *albums_ui.grid.data.lock() = data.clone();
        // The album set changed — the memoized filter+sort indices are stale.
        *albums_ui.grid.index_cache.lock() = None;
    }

    // Prewarm the first few screenfuls of grid-tier covers so the initial
    // grid paint is a cache hit. The rest decode lazily on scroll-in via
    // `request-cover` — covers are virtualized now, so prewarming the
    // whole catalogue would just thrash the grid-tier LRU on large
    // libraries (and waste CPU decoding covers the user never scrolls to).
    // `album_stats` is name-sorted, so the first `GRID_PREWARM_AHEAD`
    // albums are the ones first on screen. Runs on the runtime worker pool
    // — album-art decoding is CPU-bound; the bounded decode pool inside
    // `prewarm` parallelizes it while `spawn_blocking` keeps the runtime
    // responsive.
    //
    // Gated on the section being on screen: a background library-changed
    // tick must not re-fill a cache the user isn't looking at — it was
    // released on section exit and the re-enter handler re-warms it.
    if albums_ui.section_active() {
        let unique = first_screenful_paths(&data);
        if !unique.is_empty() {
            let thumbs = albums_ui.grid_covers.clone();
            let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&unique)).await;
        }
    }

    let albums_ui = albums_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        rebuild_grid(&ui, &albums_ui);
    });
    Ok(())
}

/// Rebuild the grid model from the cached grid data — no DB hit. Runs on
/// the UI thread (called directly from the `apply-filter` / `request-sort`
/// / `columns-changed` callbacks, which have already updated the `Albums`
/// global, and from `fetch_grid`'s UI hop). No cover decoding happens here
/// — cards pull their cover lazily via `request-cover`.
///
/// The filter+sort result is memoized in `grid.index_cache`: a pure
/// `columns-changed` (the common case while resizing the window or
/// toggling the sidebar) reuses the cached indices and only re-chunks,
/// skipping the filter walk and the sort entirely.
pub fn rebuild_grid(ui: &AppWindow, albums_ui: &AlbumsUi) {
    let g = ui.global::<Albums>();
    let sort_field = g.get_sort_field().to_string();
    let sort_dir = g.get_sort_dir().to_string();
    let filter = g.get_filter().to_string();
    let columns = g.get_columns().max(1);

    let data = albums_ui.grid.data.lock().clone();

    let rows = {
        let mut cache = albums_ui.grid.index_cache.lock();
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
    let total = i32::try_from(data.albums.len()).unwrap_or(i32::MAX);

    g.set_total_count(total);
    let model = g.get_grid_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiAlbumGridRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_grid_rows(ModelRc::new(VecModel::from(rows)));
    }
}

/// Filter + sort the grid data into a display-order list of album indices.
/// Pure / no UI state. The filter walk and the name / artist sorts read
/// `data.keys` (pre-lowercased in `fetch_grid`), so this allocates nothing
/// per album beyond the index `Vec` itself.
pub(super) fn compute_indices(
    data: &GridData,
    sort_field: &str,
    sort_dir: &str,
    filter: &str,
) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    let mut indices: Vec<usize> = if needle.is_empty() {
        (0..data.albums.len()).collect()
    } else {
        data.keys
            .iter()
            .enumerate()
            .filter(|(_, k)| k.name_lc.contains(&needle) || k.artist_lc.contains(&needle))
            .map(|(i, _)| i)
            .collect()
    };
    sort_album_indices(&mut indices, data, sort_field, sort_dir);
    indices
}

/// Chunk a display-order index list into rows of `columns` `AlbumRow`
/// cards. Pure; this is the only step a `columns-changed` rebuild has to
/// redo (the filter+sort `indices` are reused from `grid.index_cache`).
fn chunk_indices(data: &GridData, indices: &[usize], columns: i32) -> Vec<UiAlbumGridRow> {
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut rows: Vec<UiAlbumGridRow> = Vec::with_capacity(indices.len().div_ceil(cols));
    for chunk in indices.chunks(cols) {
        let cards: Vec<UiAlbumRow> = chunk
            .iter()
            .map(|&i| to_slint_album_row(&data.albums[i]))
            .collect();
        rows.push(UiAlbumGridRow {
            albums: ModelRc::from(Rc::new(VecModel::from(cards))),
        });
    }
    rows
}

/// Sort `indices` into the grid data by the chosen field. `album_stats` is
/// fetched name-ASC, so the Year / Artist sorts (and `desc` on any field)
/// must be done in memory — the DB query order is fixed. Reads the
/// pre-lowercased `data.keys`, so `sort_by_cached_key` caches `&str`
/// references rather than re-allocating a lowercased `String` per album.
fn sort_album_indices(indices: &mut [usize], data: &GridData, field: &str, dir: &str) {
    match field {
        "year" => indices.sort_by_cached_key(|&i| {
            (data.albums[i].year.unwrap_or(0), data.keys[i].name_lc.as_str())
        }),
        "artist" => indices.sort_by_cached_key(|&i| {
            (data.keys[i].artist_lc.as_str(), data.keys[i].name_lc.as_str())
        }),
        _ => indices.sort_by_cached_key(|&i| data.keys[i].name_lc.as_str()),
    }
    if dir == "desc" {
        indices.reverse();
    }
}

/// The first `GRID_PREWARM_AHEAD` distinct artwork paths in display
/// (name-sorted) order — the covers first on screen. Shared by
/// `fetch_grid` and `AlbumsUi::prewarm_visible_covers`. The cap counts
/// kept *paths*, so a run of covertless albums is walked past rather than
/// spending the budget on them.
pub(super) fn first_screenful_paths(data: &GridData) -> Vec<PathBuf> {
    crate::ui::grid_prewarm::unique_artwork_paths(
        data.albums.iter().map(|a| a.artwork_path.as_deref()),
        GRID_PREWARM_AHEAD,
    )
}

// --- Cap tuning -----------------------------------------------------------

/// Retune the grid-tier cover cache to the real display resolution. Called
/// once at startup after the winit window is live (`main.rs`); the cache is
/// constructed with `DEFAULT_GRID_COVER_CAP` and resized here. The
/// detail-tier `(cover, blur)` pair cache keeps its small fixed cap (see
/// [`crate::ui::detail_artwork`]).
pub fn tune_cache_for_display(app: &AppWindow, albums_ui: &AlbumsUi) {
    let cap = crate::ui::grid_prewarm::cover_cap_for_window(app, DEFAULT_GRID_COVER_CAP);
    albums_ui.grid_covers.resize(cap);
    log::debug!("ui::albums album-cover cache cap tuned to {cap}");
}

