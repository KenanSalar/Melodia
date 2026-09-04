//! Artists grid: DB fetch + filter / sort / chunk / prewarm logic, plus
//! the display-aware cover-cache cap tuner. Mirrors `src/ui/albums/grid.rs`.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::state::{DEFAULT_GRID_COVER_CAP, GRID_PREWARM_AHEAD, GridData, GridIndexCache};
use super::{ArtistsUi, to_slint_artist_row};
use crate::ui::grid_rows::chunk_rows;
use crate::ui::row_match;
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::AppResult;
use melodia_ui::{AppWindow, ArtistGridRow as UiArtistGridRow, Artists};

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
    let total = len_as_i32(data.artists.len());

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
    let needle = row_match::fold_needle(filter);
    let mut indices: Vec<usize> = if needle.is_empty() {
        (0..data.artists.len()).collect()
    } else {
        data.artists
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                needle.contains(&a.name)
                    || a.sort_name.as_deref().is_some_and(|s| needle.contains(s))
            })
            .map(|(i, _)| i)
            .collect()
    };
    sort_artist_indices(&mut indices, data, sort_field, sort_dir);
    indices
}

/// Chunk a display-order index list into rows of `columns` `ArtistRow`
/// cards. Pure; only step a `columns-changed` rebuild has to redo.
fn chunk_indices(data: &GridData, indices: &[usize], columns: i32) -> Vec<UiArtistGridRow> {
    chunk_rows(
        indices,
        columns,
        |&i| to_slint_artist_row(&data.artists[i]),
        |artists| UiArtistGridRow { artists },
    )
}

/// Sort `indices` into the grid data by the chosen field. Numeric sorts
/// read directly from `data.artists`; the name sort reads the pre-
/// lowercased `data.keys`.
fn sort_artist_indices(indices: &mut [usize], data: &GridData, field: &str, dir: &str) {
    match field {
        "track_count" => indices
            .sort_by_cached_key(|&i| (data.artists[i].track_count, data.keys[i].name_lc.as_str())),
        "album_count" => indices
            .sort_by_cached_key(|&i| (data.artists[i].album_count, data.keys[i].name_lc.as_str())),
        _ => indices.sort_by_cached_key(|&i| data.keys[i].name_lc.as_str()),
    }
    if dir == "desc" {
        indices.reverse();
    }
}

/// The first `GRID_PREWARM_AHEAD` distinct artist images in display
/// (name-sorted) order. The cap counts kept *paths*, which matters more
/// here than on the album grid: most artists have no image, so this walks
/// well past the first screenful to find that many rather than prewarming
/// the two or three the opening rows happen to carry.
pub(super) fn first_screenful_paths(data: &GridData) -> Vec<PathBuf> {
    crate::ui::grid_prewarm::unique_artwork_paths(
        data.artists.iter().map(|a| a.image_path.as_deref()),
        GRID_PREWARM_AHEAD,
    )
}

// --- Cap tuning -----------------------------------------------------------

/// Retune the grid-tier cover cache to the real display resolution. Called after
/// `app.show()` and again on every resize, off `WindowChrome.display-changed`; the cache is
/// constructed with `DEFAULT_GRID_COVER_CAP` and resized here. The
/// detail-tier `(cover, blur)` pair cache keeps its small fixed cap (see
/// [`crate::ui::detail_artwork`]).
pub fn tune_cache_for_display(app: &AppWindow, artists_ui: &ArtistsUi) {
    let cap = crate::ui::grid_prewarm::cover_cap_for_window(app, DEFAULT_GRID_COVER_CAP);
    let size = crate::ui::grid_prewarm::cover_size_for_window(app);
    artists_ui.grid_covers.resize(cap);
    artists_ui.grid_covers.set_thumb_size(size);
    log::debug!("ui::artists artist-cover cache tuned to cap {cap}, {size} px");
}
