//! DB fetch + filter rebuild + favourite-row in-place update.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::{
    RowSearchKey, TracksUi, finish_track_list_row, prepare_track_list_row,
};
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::media::cover_thumbs::CoverThumbs;
use crate::state::AppState;
use crate::ui::track_sort;
use crate::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

/// Re-fetch the full list from the DB, store it in `tracks_ui`, then push the
/// filtered+sorted view into the Slint model. Async — runs on the tokio
/// runtime; the UI write is hopped back via `upgrade_in_event_loop`.
///
/// Cover thumbnails for unique paths are decoded in parallel via Rayon
/// inside a `spawn_blocking` task before the rows are built, so by the
/// time `build_visible` runs the cache is fully warm and the per-row
/// `to_slint_track_list_row` calls are hashmap hits.
pub async fn fetch_and_apply(
    state: &AppState,
    tracks_ui: &TracksUi,
    weak: Weak<AppWindow>,
    sort_field: String,
    sort_dir: String,
    filter: String,
) -> AppResult<()> {
    // Fetch in a fixed order (the DB's `sort_key` default) — display order
    // is derived entirely in memory below, so the cold fetch and a later
    // header-click re-sort share the one `compute_track_order` code path.
    let rows = library::tracks::get_tracks(state, None, None).await?;
    let total = i32::try_from(rows.len()).unwrap_or(i32::MAX);

    // Pre-compute lowercase columns for the filter pass. Kept aligned by
    // index with `full`, so `full[i]` and `search_keys[i]` describe the
    // same row regardless of the display `order` permutation.
    let keys: Vec<RowSearchKey> = rows.iter().map(RowSearchKey::from_row).collect();
    let order = track_sort::compute_track_order(&rows, &sort_field, &sort_dir);
    *tracks_ui.full.lock() = Arc::new(rows);
    *tracks_ui.search_keys.lock() = Arc::new(keys);
    *tracks_ui.order.lock() = Arc::new(order);

    // Pre-warm the thumbnail cache off the runtime worker pool. Album art
    // decoding is CPU-bound; Rayon parallelizes across cores while
    // `spawn_blocking` keeps the tokio runtime responsive.
    let unique_paths: Vec<PathBuf> = {
        let full = tracks_ui.full.lock().clone();
        let cap = (full.len() / 8).max(16);
        let mut seen: HashSet<&str> = HashSet::with_capacity(cap);
        let mut out: Vec<PathBuf> = Vec::with_capacity(cap);
        for r in full.iter() {
            if let Some(p) = r.artwork_path.as_deref()
                && !p.is_empty()
                && seen.insert(p)
            {
                out.push(PathBuf::from(p));
            }
        }
        out
    };
    if !unique_paths.is_empty() {
        let thumbs = tracks_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || {
            thumbs.prewarm(&unique_paths);
        })
        .await;
    }

    let snapshot = tracks_ui.full.lock().clone();
    let keys = tracks_ui.search_keys.lock().clone();
    let order = tracks_ui.order.lock().clone();
    let thumbs = tracks_ui.cover_thumbs.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let visible = build_visible(&snapshot, &keys, &order, &filter, &thumbs);
        apply_visible(&ui, visible, total);
    });
    Ok(())
}

/// Re-derive the visible rows from `tracks_ui.full` after a filter change.
/// No DB hit. Cache is warm by this point so the per-row build is cheap.
pub fn refilter(weak: &Weak<AppWindow>, tracks_ui: &TracksUi, filter: String) {
    let snapshot = tracks_ui.full.lock().clone();
    let keys = tracks_ui.search_keys.lock().clone();
    let order = tracks_ui.order.lock().clone();
    let total = i32::try_from(snapshot.len()).unwrap_or(i32::MAX);
    let thumbs = tracks_ui.cover_thumbs.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let visible = build_visible(&snapshot, &keys, &order, &filter, &thumbs);
        apply_visible(&ui, visible, total);
    });
}

/// Re-sort the Tracks view after a header-column click. Fully in memory:
/// recomputes only the `order` index permutation — no DB round-trip and
/// no `RowSearchKey` rebuild (a sort change reorders rows but the
/// `(row, key)` set is unchanged). `full` / `search_keys` stay untouched.
///
/// Called from `on_request_sort` (already on the UI thread); the
/// `compute_track_order` sort runs inline, then the model write hops
/// through `upgrade_in_event_loop` exactly like `refilter`.
pub fn resort_and_apply(
    weak: &Weak<AppWindow>,
    tracks_ui: &TracksUi,
    sort_field: &str,
    sort_dir: &str,
    filter: String,
) {
    let snapshot = tracks_ui.full.lock().clone();
    let keys = tracks_ui.search_keys.lock().clone();
    let order = Arc::new(track_sort::compute_track_order(&snapshot, sort_field, sort_dir));
    *tracks_ui.order.lock() = order.clone();
    let total = i32::try_from(snapshot.len()).unwrap_or(i32::MAX);
    let thumbs = tracks_ui.cover_thumbs.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let visible = build_visible(&snapshot, &keys, &order, &filter, &thumbs);
        apply_visible(&ui, visible, total);
    });
}

/// Flip `is_favorite` on a single row in both the canonical Vec and the
/// Slint `VecModel`. Only touches the affected row — scroll position and
/// neighbouring rows stay put.
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let rows = ui.global::<Tracks>().get_rows();
        let Some(vec_model) =
            rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>()
        else {
            return;
        };
        for i in 0..vec_model.row_count() {
            let Some(mut r) = vec_model.row_data(i) else {
                continue;
            };
            if i64::from(r.id) == id {
                r.is_favorite = fav;
                vec_model.set_row_data(i, r);
                break;
            }
        }
    });
}

/// Filter and convert `full` into UI rows, walking `order` so rows come
/// out in the current display sort order. Pure / does not touch any UI
/// state — safe to run on either the runtime worker (cold path, after
/// `prewarm`) or the UI thread (warm cache, refilter / resort).
///
/// `full.get(i)` / `keys.get(i)` keep this panic-safe if a concurrent
/// fetch swapped `full` between the snapshot locks — a stale `order`
/// index is simply skipped, and the fetch's own rebuild restores it.
fn build_visible(
    full: &[RsTrackListRow],
    keys: &[RowSearchKey],
    order: &[usize],
    filter: &str,
    thumbs: &CoverThumbs,
) -> Vec<UiTrackListRow> {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return order
            .iter()
            .filter_map(|&i| {
                let r = full.get(i)?;
                Some(finish_track_list_row(prepare_track_list_row(r), thumbs))
            })
            .collect();
    }
    order
        .iter()
        .filter_map(|&i| {
            let r = full.get(i)?;
            keys.get(i)?
                .matches(&needle)
                .then(|| finish_track_list_row(prepare_track_list_row(r), thumbs))
        })
        .collect()
}

/// UI-thread-only: apply current selection state to `visible` and swap
/// the model contents via the shared diff helper, which prefers per-row
/// `set_row_data` over `set_vec` when ids align positionally so the
/// `ListView`'s delegate cache survives.
fn apply_visible(ui: &AppWindow, mut visible: Vec<UiTrackListRow>, total: i32) {
    let g = ui.global::<Tracks>();
    let selected_set: HashSet<i32> = g.get_selected_ids().iter().collect();
    for row in &mut visible {
        if selected_set.contains(&row.id) {
            row.selected = true;
        }
    }
    g.set_total_count(total);
    let rows = g.get_rows();
    let Some(vec_model) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        return;
    };
    crate::ui::model_diff::apply_rows_keyed(vec_model, visible, |r| r.id);
}
