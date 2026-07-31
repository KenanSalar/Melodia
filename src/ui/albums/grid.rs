//! Albums grid: DB fetch + filter / sort / chunk / prewarm logic, plus the
//! display-aware cover-cache cap tuner.

use std::num::NonZeroUsize;
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

/// The deduplicated artwork paths of the first `GRID_PREWARM_AHEAD`
/// (name-sorted) albums — the covers first on screen. Shared by
/// `fetch_grid` and `AlbumsUi::prewarm_visible_covers`.
pub(super) fn first_screenful_paths(data: &GridData) -> Vec<PathBuf> {
    crate::ui::grid_prewarm::unique_artwork_paths(
        data.albums.iter().map(|a| a.artwork_path.as_deref()),
        GRID_PREWARM_AHEAD,
    )
}

// --- Cap tuning -----------------------------------------------------------

/// Estimate a sensible grid-cover LRU capacity for a display of the given
/// *logical* (DPI-divided) pixel dimensions. The grid virtualizes by row,
/// so the working set is "cards visible at once" — a bigger panel shows
/// more.
///
/// The flex-filled grid cards are *large* (the user runs them well past
/// 200 px), so this uses a generous footprint (~260 px wide incl. gap,
/// ~320 px tall incl. text + gap) — a smaller footprint over-counts what's
/// really on screen. `rows` adds one partial row as the only scroll-back
/// headroom: no extra multiplier, because even fullscreen at 1440p only
/// ~50 cards are visible at once, so a 1.5× cushion was just dead weight.
/// Clamped to `[32, 96]` — at 448 px / ~600 KB per entry that's a
/// ~19–58 MB band, and the cache is released entirely when the user
/// leaves the section anyway. The footprint constants and clamps are the
/// tunable knobs. Lands ≈ 1080p → 35, 1440p → 54, 4K → 96.
pub(super) fn compute_album_cover_cap(logical_w: u32, logical_h: u32) -> NonZeroUsize {
    const CARD_FOOTPRINT_W: u32 = 260;
    const ROW_FOOTPRINT_H: u32 = 320;
    const MIN_CAP: usize = 32;
    const MAX_CAP: usize = 96;

    let cols = (logical_w / CARD_FOOTPRINT_W).max(1);
    // `+ 1` for the partially-visible row — the only scroll headroom.
    let rows = logical_h.div_ceil(ROW_FOOTPRINT_H) + 1;
    let visible = usize::try_from(cols.saturating_mul(rows)).unwrap_or(MAX_CAP);
    let cap = visible.clamp(MIN_CAP, MAX_CAP);
    NonZeroUsize::new(cap).unwrap_or(DEFAULT_GRID_COVER_CAP)
}

/// Convert a physical pixel extent + DPI scale into a logical extent.
/// Saturating boundary for the `f64 → u32` step — mirrors
/// `media::artwork::f64_to_pixel`; monitor extents stay far below
/// `u32::MAX` in practice.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "logical screen extent stays well below u32::MAX; this is the saturating boundary"
)]
fn logical_dim(physical: u32, scale: f64) -> u32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let v = (f64::from(physical) / scale).round();
    if v.is_nan() || v <= 0.0 {
        physical
    } else if v >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        v as u32
    }
}

/// Query the window's current monitor and derive a grid-cover cap from
/// its logical resolution. Falls back to `DEFAULT_GRID_COVER_CAP` when
/// the monitor can't be read (e.g. some Wayland setups report `None`).
fn album_cover_cap_for_window(app: &AppWindow) -> NonZeroUsize {
    use slint::winit_030::WinitWindowAccessor;

    app.window()
        .with_winit_window(|w| {
            let monitor = w.current_monitor()?;
            let physical = monitor.size();
            let scale = w.scale_factor();
            Some(compute_album_cover_cap(
                logical_dim(physical.width, scale),
                logical_dim(physical.height, scale),
            ))
        })
        .flatten()
        .unwrap_or(DEFAULT_GRID_COVER_CAP)
}

/// Retune the grid-tier cover cache to the real display resolution.
/// Called once at startup after the winit window is live (`main.rs`); the
/// cache is constructed with `DEFAULT_GRID_COVER_CAP` and resized here.
/// The detail-tier `(cover, blur)` pair cache keeps its small fixed cap
/// (see [`crate::ui::detail_artwork`]).
pub fn tune_cache_for_display(app: &AppWindow, albums_ui: &AlbumsUi) {
    let cap = album_cover_cap_for_window(app);
    albums_ui.grid_covers.resize(cap);
    log::debug!("ui::albums album-cover cache cap tuned to {cap}");
}
