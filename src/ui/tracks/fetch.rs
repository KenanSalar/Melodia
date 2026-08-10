//! DB fetch + filter rebuild + favourite-row in-place update.

use std::collections::HashSet;
use std::path::PathBuf;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::TracksUi;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::model_patch;
use crate::ui::row_match::fold_needle;
use crate::ui::track_list_cache::CacheData;
use crate::{AppWindow, TrackListRow as UiTrackListRow, Tracks};

/// Re-fetch the full list from the DB, store it in `tracks_ui`, then push the
/// filtered+sorted view into the Slint model. Async — runs on the tokio
/// runtime; the UI write is hopped back via `upgrade_in_event_loop`.
///
/// Cover thumbnails for unique paths are decoded in parallel via Rayon
/// inside a `spawn_blocking` task, so the rows the `ListView` mounts find a
/// warm tier instead of each dragging a decode onto the UI thread.
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

    // Consumes `rows`: each DB row's strings are freed as its converted row
    // is built, so the peak never holds the library's text twice.
    tracks_ui.cache.store(rows, &sort_field, &sort_dir);

    // Pre-warm the thumbnail cache off the runtime worker pool. Album art
    // decoding is CPU-bound; Rayon parallelizes across cores while
    // `spawn_blocking` keeps the tokio runtime responsive. Walked in
    // DISPLAY order so that on a library with more unique covers than the
    // cache holds, the entries surviving the cap are the ones the user sees
    // first.
    let snapshot = tracks_ui.cache.snapshot();
    let unique_paths: Vec<PathBuf> = snapshot.artwork_paths(tracks_ui.cover_thumbs.capacity());
    if !unique_paths.is_empty() {
        let thumbs = tracks_ui.cover_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || {
            thumbs.prewarm(&unique_paths);
        })
        .await;
    }

    apply(&weak, &snapshot, &filter);
    Ok(())
}

/// Re-derive the visible rows after a filter change. No DB hit, and no
/// allocation per row — the cached rows are cloned, which is a refcount bump
/// per `SharedString`.
pub fn refilter(weak: &Weak<AppWindow>, tracks_ui: &TracksUi, filter: &str) {
    apply(weak, &tracks_ui.cache.snapshot(), filter);
}

/// Re-sort the Tracks view after a header-column click. Fully in memory:
/// recomputes only the display permutation — no DB round-trip and no key
/// rebuild, since a sort change reorders rows but leaves the `(row, key)`
/// set alone.
///
/// Called from `on_request_sort` (already on the UI thread); the sort runs
/// inline, then the model write hops through `upgrade_in_event_loop` exactly
/// like `refilter`.
pub fn resort_and_apply(
    weak: &Weak<AppWindow>,
    tracks_ui: &TracksUi,
    sort_field: &str,
    sort_dir: &str,
    filter: &str,
) {
    apply(weak, &tracks_ui.cache.resort(sort_field, sort_dir), filter);
}

/// Filter `snapshot` into display order and swap the result into the model.
///
/// The walk runs on the **calling** thread — the runtime worker on the cold
/// fetch path, the UI thread on a refilter or resort, which is where those
/// two already are. Only the finished `Vec` crosses the hop, and moving one
/// is a pointer move; deferring the walk instead would put the cold path's
/// whole rebuild on the event loop.
fn apply(weak: &Weak<AppWindow>, snapshot: &CacheData, filter: &str) {
    let visible = snapshot.visible(&fold_needle(filter));
    let total = snapshot.total();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        apply_visible(&ui, visible, total);
    });
}

/// Flip `is_favorite` on a single row in the Slint `VecModel`. Only touches
/// the affected row — scroll position and neighbouring rows stay put.
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Tracks>().get_rows(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// Set `rating` on a single row in the Slint `VecModel` — the star-rating
/// analogue of [`apply_row_favorite`].
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Tracks>().get_rows(), id, |r| {
            r.rating = rating;
        });
    });
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
