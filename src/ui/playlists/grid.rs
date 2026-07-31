//! Playlists grid: DB fetch + filter / sort / chunk / prewarm logic, plus
//! the display-aware cover-cache cap tuner. Mirrors `albums::grid`.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::state::{
    DEFAULT_GRID_COVER_CAP, GRID_PREWARM_AHEAD, GridData, GridIndexCache,
};
use super::{PlaylistsUi, to_slint_playlist_row};
use crate::entities::smart_criteria::SmartCriteria;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::{
    AppWindow, PlaylistGridRow as UiPlaylistGridRow, PlaylistRow as UiPlaylistRow, Playlists,
};

/// How much smart-playlist counting a grid refresh pays for.
#[derive(Clone, Copy)]
enum RefreshKind {
    /// Recount every smart playlist — structure may have changed (scan,
    /// watcher, CRUD, criteria edit).
    Full,
    /// A play-count-flush-driven refresh: recount only smart playlists whose
    /// criteria depend on play stats; carry the rest forward from the previous
    /// grid data (a criteria edit bumps `library_changed`, forcing a `Full`
    /// refresh, so the prior stat-dependence flags stay valid here).
    StatsOnly,
}

/// Fetch the playlist list from the DB, prewarm cover thumbnails, then
/// rebuild the grid model on the UI thread. The flat `Playlists.rows`
/// model (used by the Add-to-Playlist submenu) is repopulated in the same
/// UI hop so the submenu stays in sync with the grid.
pub async fn fetch_grid(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
) -> AppResult<()> {
    fetch_grid_inner(state, playlists_ui, weak, RefreshKind::Full).await
}

/// `stats_changed`-driven grid refresh: recount only the stat-dependent smart
/// playlists (others keep their previous counts, which a play stat change can't
/// move). Gated upstream by `has_stat_dependent_smart_playlists`.
pub async fn fetch_grid_stats(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
) -> AppResult<()> {
    fetch_grid_inner(state, playlists_ui, weak, RefreshKind::StatsOnly).await
}

async fn fetch_grid_inner(
    state: &AppState,
    playlists_ui: &Arc<PlaylistsUi>,
    weak: Weak<AppWindow>,
    kind: RefreshKind,
) -> AppResult<()> {
    let mut playlists = library::playlists::get_playlists(state).await?;

    // Smart playlists have no `playlist_items` rows, so the trigger-maintained
    // `track_count` / `total_duration_ms` stay 0 — resolve them from the stored
    // criteria. On a `StatsOnly` refresh only the stat-dependent rows can have
    // changed, so the rest carry their previous counts forward (the previous
    // grid data is the source, cloned once up front — a cheap `Arc` bump).
    //
    // Transient-staleness note: `prev` is snapshotted and the counts computed
    // before `section.gate()` is acquired, so the gate serializes only the final
    // write (against the wipe and sibling writes), not the snapshot→compute→write
    // as a whole — the final write is therefore last-writer-wins. If a
    // `library_changed`-driven `Full` refresh (e.g. a scan that changed a
    // stat-*independent* smart playlist's membership) commits while this
    // `StatsOnly` pass is mid-flight, the count carried forward for that row can
    // be briefly stale. Display-only, and it self-heals on the next `Full`
    // refresh (next `library_changed` / section re-enter).
    let prev = playlists_ui.grid.data.lock().clone();
    let mut to_count: Vec<usize> = Vec::new();
    for (i, p) in playlists.iter_mut().enumerate() {
        if !p.is_smart {
            continue;
        }
        let recount = match kind {
            RefreshKind::Full => true,
            RefreshKind::StatsOnly => match prev.row_stats_by_id(p.id) {
                Some((count, duration, stat_dependent)) if !stat_dependent => {
                    // A play stat change can't move this row — carry forward.
                    p.track_count = count;
                    p.total_duration_ms = duration;
                    false
                }
                // Stat-dependent, or absent from the previous data → recount.
                _ => true,
            },
        };
        if recount {
            to_count.push(i);
        }
    }

    // Parse each selected row's criteria once, then fan the COUNT/SUM queries
    // out across the read pool (WAL allows concurrent readers) instead of
    // awaiting them serially. Results are written back by index — order-
    // independent — so `join_all` completion order doesn't matter.
    let parsed: Vec<(usize, SmartCriteria)> = to_count
        .iter()
        .map(|&i| {
            (
                i,
                SmartCriteria::from_json_opt(playlists[i].smart_criteria.as_deref()),
            )
        })
        .collect();
    let counts = futures_util::future::join_all(
        parsed
            .iter()
            .map(|(i, criteria)| async move { (*i, library::smart_playlists::count(state, criteria).await) }),
    )
    .await;
    for (i, result) in counts {
        match result {
            Ok((count, duration)) => {
                playlists[i].track_count = i32::try_from(count).unwrap_or(i32::MAX);
                playlists[i].total_duration_ms = duration;
            }
            Err(e) => log::warn!("smart playlist {} count failed: {e}", playlists[i].id),
        }
    }

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
    crate::ui::grid_prewarm::unique_artwork_paths(
        data.playlists.iter().map(|p| p.thumbnail_path.as_deref()),
        GRID_PREWARM_AHEAD,
    )
}

// --- Cap tuning -----------------------------------------------------------

/// Retune the grid-tier cover cache to the real display resolution. Called
/// once at startup after the winit window is live (`main.rs`); the cache is
/// constructed with `DEFAULT_GRID_COVER_CAP` and resized here. The
/// detail-tier `(cover, blur)` pair cache keeps its small fixed cap (see
/// [`crate::ui::detail_artwork`]).
pub fn tune_cache_for_display(app: &AppWindow, playlists_ui: &PlaylistsUi) {
    let cap = crate::ui::grid_prewarm::cover_cap_for_window(app, DEFAULT_GRID_COVER_CAP);
    playlists_ui.grid_covers.resize(cap);
    log::debug!("ui::playlists playlist-cover cache cap tuned to {cap}");
}

