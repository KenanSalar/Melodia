//! Artists grid: DB fetch + filter / sort / chunk / prewarm logic, plus
//! the display-aware cover-cache cap tuner. Mirrors `src/ui/albums/grid.rs`.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::state::{
    DEFAULT_GRID_COVER_CAP, GRID_PREWARM_AHEAD, GridData, GridIndexCache,
};
use super::{ArtistsUi, to_slint_artist_row};
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::{
    AppWindow, ArtistGridRow as UiArtistGridRow, ArtistRow as UiArtistRow, Artists,
};

/// Fetch the artist list from the DB into `artists_ui.grid.data`, prewarm
/// cover thumbnails, then rebuild the grid model on the UI thread.
pub async fn fetch_grid(
    state: &AppState,
    artists_ui: &Arc<ArtistsUi>,
    weak: Weak<AppWindow>,
) -> AppResult<()> {
    let artists = library::artists::get_artists(state).await?;
    let data = Arc::new(GridData::new(artists));
    // See `ui::albums::grid::fetch_grid` for the gate rationale.
    {
        let _gate = artists_ui.section.gate();
        *artists_ui.grid.data.lock() = data.clone();
        *artists_ui.grid.index_cache.lock() = None;
    }

    if artists_ui.section_active() {
        let unique = first_screenful_paths(&data);
        if !unique.is_empty() {
            let thumbs = artists_ui.grid_covers.clone();
            let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&unique)).await;
        }
    }

    let artists_ui = artists_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        rebuild_grid(&ui, &artists_ui);
    });
    Ok(())
}

/// Rebuild the grid model from the cached grid data — no DB hit. Runs on
/// the UI thread.
pub fn rebuild_grid(ui: &AppWindow, artists_ui: &ArtistsUi) {
    let g = ui.global::<Artists>();
    let sort_field = g.get_sort_field().to_string();
    let sort_dir = g.get_sort_dir().to_string();
    let filter = g.get_filter().to_string();
    let columns = g.get_columns().max(1);

    let data = artists_ui.grid.data.lock().clone();

    let rows = {
        let mut cache = artists_ui.grid.index_cache.lock();
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
    let total = i32::try_from(data.artists.len()).unwrap_or(i32::MAX);

    g.set_total_count(total);
    let model = g.get_grid_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiArtistGridRow>>() {
        vm.set_vec(rows);
    } else {
        g.set_grid_rows(ModelRc::new(VecModel::from(rows)));
    }
}

/// Filter + sort the grid data into a display-order list of artist
/// indices. Pure / no UI state.
pub(super) fn compute_indices(
    data: &GridData,
    sort_field: &str,
    sort_dir: &str,
    filter: &str,
) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    let mut indices: Vec<usize> = if needle.is_empty() {
        (0..data.artists.len()).collect()
    } else {
        data.keys
            .iter()
            .enumerate()
            .filter(|(_, k)| k.name_lc.contains(&needle) || k.sort_name_lc.contains(&needle))
            .map(|(i, _)| i)
            .collect()
    };
    sort_artist_indices(&mut indices, data, sort_field, sort_dir);
    indices
}

/// Chunk a display-order index list into rows of `columns` `ArtistRow`
/// cards. Pure; only step a `columns-changed` rebuild has to redo.
fn chunk_indices(data: &GridData, indices: &[usize], columns: i32) -> Vec<UiArtistGridRow> {
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut rows: Vec<UiArtistGridRow> = Vec::with_capacity(indices.len().div_ceil(cols));
    for chunk in indices.chunks(cols) {
        let cards: Vec<UiArtistRow> = chunk
            .iter()
            .map(|&i| to_slint_artist_row(&data.artists[i]))
            .collect();
        rows.push(UiArtistGridRow {
            artists: ModelRc::from(Rc::new(VecModel::from(cards))),
        });
    }
    rows
}

/// Sort `indices` into the grid data by the chosen field. Numeric sorts
/// read directly from `data.artists`; the name sort reads the pre-
/// lowercased `data.keys`.
fn sort_artist_indices(indices: &mut [usize], data: &GridData, field: &str, dir: &str) {
    match field {
        "track_count" => indices.sort_by_cached_key(|&i| {
            (data.artists[i].track_count, data.keys[i].name_lc.as_str())
        }),
        "album_count" => indices.sort_by_cached_key(|&i| {
            (data.artists[i].album_count, data.keys[i].name_lc.as_str())
        }),
        _ => indices.sort_by_cached_key(|&i| data.keys[i].name_lc.as_str()),
    }
    if dir == "desc" {
        indices.reverse();
    }
}

/// The deduplicated artwork paths of the first `GRID_PREWARM_AHEAD`
/// (name-sorted) artists' covers — the ones first on screen.
pub(super) fn first_screenful_paths(data: &GridData) -> Vec<PathBuf> {
    crate::ui::grid_prewarm::unique_artwork_paths(
        data.artists.iter().map(|a| a.image_path.as_deref()),
        GRID_PREWARM_AHEAD,
    )
}

// --- Cap tuning -----------------------------------------------------------

/// Estimate a sensible grid-cover LRU capacity for a display of the given
/// *logical* pixel dimensions. Same shape as
/// `albums::grid::compute_album_cover_cap` — artist cards land at roughly
/// the same on-screen size, so the tunables match.
pub(super) fn compute_artist_cover_cap(logical_w: u32, logical_h: u32) -> NonZeroUsize {
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

fn artist_cover_cap_for_window(app: &AppWindow) -> NonZeroUsize {
    use slint::winit_030::WinitWindowAccessor;

    app.window()
        .with_winit_window(|w| {
            let monitor = w.current_monitor()?;
            let physical = monitor.size();
            let scale = w.scale_factor();
            Some(compute_artist_cover_cap(
                logical_dim(physical.width, scale),
                logical_dim(physical.height, scale),
            ))
        })
        .flatten()
        .unwrap_or(DEFAULT_GRID_COVER_CAP)
}

/// Retune the grid-tier cover cache to the real display resolution.
pub fn tune_cache_for_display(app: &AppWindow, artists_ui: &ArtistsUi) {
    let cap = artist_cover_cap_for_window(app);
    artists_ui.grid_covers.resize(cap);
    log::debug!("ui::artists artist-cover cache cap tuned to {cap}");
}
