//! Playlists grid: DB fetch + filter / sort / chunk / prewarm logic, plus
//! the display-aware cover-cache cap tuner. Mirrors `albums::grid`.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::state::{
    DEFAULT_GRID_COVER_CAP, GRID_PREWARM_AHEAD, GridData, GridIndexCache,
};
use super::{PlaylistsUi, to_slint_playlist_row};
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::{
    AppWindow, PlaylistGridRow as UiPlaylistGridRow, PlaylistRow as UiPlaylistRow, Playlists,
};

/// Fetch the playlist list from the DB, prewarm cover thumbnails, then
/// rebuild the grid model on the UI thread. The flat `Playlists.rows`
/// model (used by the Add-to-Playlist submenu) is repopulated in the same
/// UI hop so the submenu stays in sync with the grid.
pub async fn fetch_grid(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
) -> AppResult<()> {
    let playlists = library::playlists::get_playlists(state).await?;
    let data = Arc::new(GridData::new(playlists));
    {
        let _gate = playlists_ui.section.gate();
        *playlists_ui.grid.data.lock() = data.clone();
        *playlists_ui.grid.index_cache.lock() = None;
    }

    if playlists_ui.section_active() {
        let unique = first_screenful_paths(&data);
        if !unique.is_empty() {
            let thumbs = playlists_ui.grid_covers.clone();
            let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&unique)).await;
        }
    }

    let playlists_ui = playlists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        rebuild_grid(&ui, &playlists_ui);
        update_flat_rows(&ui, &playlists_ui);
    });
    Ok(())
}

/// Rebuild the grid model from the cached grid data — no DB hit. Same
/// memoization contract as `albums::grid::rebuild_grid`.
pub fn rebuild_grid(ui: &AppWindow, playlists_ui: &PlaylistsUi) {
    let g = ui.global::<Playlists>();
    let sort_field = g.get_sort_field().to_string();
    let sort_dir = g.get_sort_dir().to_string();
    let filter = g.get_filter().to_string();
    let columns = g.get_columns().max(1);

    let data = playlists_ui.grid.data.lock().clone();

    let rows = {
        let mut cache = playlists_ui.grid.index_cache.lock();
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
        let indices = cache.as_ref().map_or(&[][..], |c| c.indices.as_slice());
        chunk_indices(&data, indices, columns)
    };
    let total = i32::try_from(data.playlists.len()).unwrap_or(i32::MAX);

    g.set_total_count(total);
    let model = g.get_grid_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiPlaylistGridRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_grid_rows(ModelRc::new(VecModel::from(rows)));
    }
}

/// Rebuild the flat `Playlists.rows` model (used by the per-track
/// Add-to-Playlist submenu). Always in `updated_at DESC` order so the
/// submenu lists most-recently-touched playlists first, regardless of
/// the grid's current sort selection.
pub fn update_flat_rows(ui: &AppWindow, playlists_ui: &PlaylistsUi) {
    let g = ui.global::<Playlists>();
    let data = playlists_ui.grid.data.lock().clone();
    let rows: Vec<UiPlaylistRow> = data.playlists.iter().map(to_slint_playlist_row).collect();
    let model = g.get_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiPlaylistRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_rows(ModelRc::new(VecModel::from(rows)));
    }
}

pub(super) fn compute_indices(
    data: &GridData,
    sort_field: &str,
    sort_dir: &str,
    filter: &str,
) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    let mut indices: Vec<usize> = if needle.is_empty() {
        (0..data.playlists.len()).collect()
    } else {
        data.keys
            .iter()
            .enumerate()
            .filter(|(_, k)| k.name_lc.contains(&needle))
            .map(|(i, _)| i)
            .collect()
    };
    sort_playlist_indices(&mut indices, data, sort_field, sort_dir);
    indices
}

fn chunk_indices(data: &GridData, indices: &[usize], columns: i32) -> Vec<UiPlaylistGridRow> {
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut rows: Vec<UiPlaylistGridRow> = Vec::with_capacity(indices.len().div_ceil(cols));
    for chunk in indices.chunks(cols) {
        let cards: Vec<UiPlaylistRow> = chunk
            .iter()
            .map(|&i| to_slint_playlist_row(&data.playlists[i]))
            .collect();
        rows.push(UiPlaylistGridRow {
            playlists: ModelRc::from(Rc::new(VecModel::from(cards))),
        });
    }
    rows
}

/// Sort `indices` into the grid data by the chosen field. `playlist_stats`
/// is fetched in `updated_at DESC` order, so any other sort must be done
/// in memory.
///
/// Supported fields:
/// * `"name"` — case-insensitive, with name as the deterministic tiebreaker.
/// * `"track_count"` — primary key; name (asc) as a stable tiebreaker.
/// * `"updated"` (default) — `updated_at` RFC3339 string (lex = chrono).
fn sort_playlist_indices(indices: &mut [usize], data: &GridData, field: &str, dir: &str) {
    match field {
        "track_count" => indices.sort_by_cached_key(|&i| {
            (data.playlists[i].track_count, data.keys[i].name_lc.clone())
        }),
        "name" => indices.sort_by_cached_key(|&i| data.keys[i].name_lc.clone()),
        // "updated" and anything unrecognised fall through to the default
        // (most-recently-updated first).
        _ => indices.sort_by_cached_key(|&i| data.playlists[i].updated_at.clone()),
    }
    if dir == "desc" {
        indices.reverse();
    }
}

pub(super) fn first_screenful_paths(data: &GridData) -> Vec<PathBuf> {
    unique_artwork_paths(
        data.playlists
            .iter()
            .take(GRID_PREWARM_AHEAD)
            .map(|p| p.thumbnail_path.as_deref()),
    )
}

pub(super) fn unique_artwork_paths<'a>(
    paths: impl Iterator<Item = Option<&'a str>>,
) -> Vec<PathBuf> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths.flatten() {
        if !p.is_empty() && seen.insert(p) {
            out.push(PathBuf::from(p));
        }
    }
    out
}

// --- Cap tuning -----------------------------------------------------------

pub(super) fn compute_playlist_cover_cap(logical_w: u32, logical_h: u32) -> NonZeroUsize {
    const CARD_FOOTPRINT_W: u32 = 260;
    const ROW_FOOTPRINT_H: u32 = 320;
    const MIN_CAP: usize = 32;
    const MAX_CAP: usize = 96;

    let cols = (logical_w / CARD_FOOTPRINT_W).max(1);
    let rows = logical_h.div_ceil(ROW_FOOTPRINT_H) + 1;
    let visible = usize::try_from(cols.saturating_mul(rows)).unwrap_or(MAX_CAP);
    let cap = visible.clamp(MIN_CAP, MAX_CAP);
    NonZeroUsize::new(cap).unwrap_or(DEFAULT_GRID_COVER_CAP)
}

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

fn playlist_cover_cap_for_window(app: &AppWindow) -> NonZeroUsize {
    use slint::winit_030::WinitWindowAccessor;

    app.window()
        .with_winit_window(|w| {
            let monitor = w.current_monitor()?;
            let physical = monitor.size();
            let scale = w.scale_factor();
            Some(compute_playlist_cover_cap(
                logical_dim(physical.width, scale),
                logical_dim(physical.height, scale),
            ))
        })
        .flatten()
        .unwrap_or(DEFAULT_GRID_COVER_CAP)
}

pub fn tune_cache_for_display(app: &AppWindow, playlists_ui: &PlaylistsUi) {
    let cap = playlist_cover_cap_for_window(app);
    playlists_ui.grid_covers.resize(cap);
    log::debug!("ui::playlists playlist-cover cache cap tuned to {cap}");
}
