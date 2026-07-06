//! Folder fetch + sort + favourite-row update. Bumps `fetch_token` to
//! drop stale UI writes when a faster navigation overtakes us.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::breadcrumbs::{build_breadcrumbs, folder_basename, sort_browse_files};
use super::models::{replace_breadcrumb_model, replace_folder_model, replace_rows_model};
use super::selection::{apply_selection_to_rows, reset_selection};
use super::{BrowseUi, to_slint_browse_track_row};
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::{
    AppWindow, Browse, BrowseFolderRow as UiBrowseFolderRow, TrackListRow as UiTrackListRow,
};

/// Re-fetch the current folder (root or otherwise) and push the result
/// into the Slint models. Async — runs on the tokio runtime; the UI
/// write hops back via `upgrade_in_event_loop`.
///
/// Bumps `fetch_token` and captures its post-bump value; if a later
/// fetch overtakes us by the time the UI-thread closure runs, the late
/// fetch drops its UI write (the more recent one already painted).
///
/// Navigation clears the selection — keeping selected ids across a
/// folder change would risk stale ids referencing rows that no longer
/// exist.
pub async fn fetch_and_apply(
    state: &AppState,
    browse_ui: &Arc<BrowseUi>,
    weak: Weak<AppWindow>,
    path: String,
) -> AppResult<()> {
    let my_token = browse_ui.fetch_token.fetch_add(1, Ordering::Relaxed) + 1;

    // Flip loading on (UI thread). Tolerates a missed token-bump race;
    // even if a later fetch overtakes us, painting `loading: true`
    // briefly is harmless.
    {
        let weak2 = weak.clone();
        let _ = weak2.upgrade_in_event_loop(|ui| {
            ui.global::<Browse>().set_loading(true);
        });
    }

    if path.is_empty() {
        // Root view: render the library folder list as drillable rows.
        let folders = library::settings::get_folders(state).await?;
        let token = browse_ui.fetch_token.load(Ordering::Relaxed);
        if token != my_token {
            return Ok(());
        }

        let ui_folders: Vec<UiBrowseFolderRow> = folders
            .iter()
            .filter(|f| f.is_enabled)
            .map(|f| UiBrowseFolderRow {
                name: SharedString::from(folder_basename(&f.path)),
                path: SharedString::from(f.path.as_str()),
            })
            .collect();
        let has_library_folders = !ui_folders.is_empty();

        *browse_ui.last_files.lock() = Vec::new();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let g = ui.global::<Browse>();
            replace_folder_model(&g, ui_folders);
            replace_rows_model(&g, Vec::new());
            replace_breadcrumb_model(&g, Vec::new());
            reset_selection(&g);
            g.set_current_path(SharedString::from(""));
            g.set_has_library_folders(has_library_folders);
            g.set_has_playable_files(false);
            g.set_can_go_back(false);
            g.set_error_message(SharedString::from(""));
            g.set_loading(false);
        });
        return Ok(());
    }

    // Drilled-in view: fetch via `browse_directory`. On Err (folder
    // deleted, permission denied, etc.) paint the error state — don't
    // propagate up, because the watcher-driven re-fetch path doesn't
    // want a transient FS hiccup to disable the whole subscriber.
    //
    // Fetch the library-folder list once and reuse it: `browse_directory`
    // needs it to validate the path is inside an enabled folder, and
    // `build_breadcrumbs` needs it to truncate the trail at the library
    // root. Previously each re-queried it independently — two identical
    // full-table `folders` reads per navigation.
    let library_folders = library::settings::get_folders(state)
        .await
        .unwrap_or_default();
    let result =
        library::browse::browse_directory(state, path.clone(), &library_folders).await;

    let token = browse_ui.fetch_token.load(Ordering::Relaxed);
    if token != my_token {
        return Ok(());
    }

    match result {
        Ok(res) => {
            // Prewarm cover thumbnails. Most files in a single folder
            // share an album cover, so unique paths are typically a small
            // fraction of total rows — pre-size the dedupe accordingly.
            let unique_paths: Vec<PathBuf> = {
                let cap = (res.files.len() / 4).max(8);
                let mut seen: HashSet<&str> = HashSet::with_capacity(cap);
                let mut out: Vec<PathBuf> = Vec::with_capacity(cap);
                for f in &res.files {
                    if let Some(p) = f.row.artwork_path.as_deref()
                        && !p.is_empty()
                        && seen.insert(p)
                    {
                        out.push(PathBuf::from(p));
                    }
                }
                out
            };
            if !unique_paths.is_empty() {
                let thumbs = browse_ui.cover_thumbs.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    thumbs.prewarm(&unique_paths);
                })
                .await;
            }

            let token = browse_ui.fetch_token.load(Ordering::Relaxed);
            if token != my_token {
                return Ok(());
            }

            // Sort to the user's current order. The sorted list is cached
            // in `last_files` (callbacks like `play-row` / selection read
            // it without round-tripping) — but the caching happens inside
            // the UI closure below as a *move*, not a `clone_from`.
            let sort_field = browse_ui.sort_field();
            let sort_dir = browse_ui.sort_dir();
            let mut files = res.files;
            sort_browse_files(&mut files, &sort_field, &sort_dir);

            let ui_folders: Vec<UiBrowseFolderRow> = res
                .folders
                .iter()
                .map(|f| UiBrowseFolderRow {
                    name: SharedString::from(f.name.as_str()),
                    path: SharedString::from(f.path.as_str()),
                })
                .collect();
            let breadcrumbs = build_breadcrumbs(&res.path, &library_folders);
            let has_playable = files.iter().any(|f| f.in_library);
            let can_go_back = !browse_ui.history.lock().is_empty();
            let current_path = res.path.clone();
            let browse_ui = browse_ui.clone();

            let _ = weak.upgrade_in_event_loop(move |ui| {
                let g = ui.global::<Browse>();
                // Build the rows from `&files`, then move `files` itself
                // into the `last_files` cache as the final step — one
                // move, no clone (the old `clone_from` deep-cloned the
                // whole `Vec<BrowseFile>` a second time). Covers resolve
                // lazily per visible row via `RowCovers.request`.
                let ui_rows: Vec<UiTrackListRow> = files
                    .iter()
                    .map(to_slint_browse_track_row)
                    .collect();
                replace_folder_model(&g, ui_folders);
                replace_rows_model(&g, ui_rows);
                replace_breadcrumb_model(&g, breadcrumbs);
                reset_selection(&g);
                g.set_current_path(SharedString::from(current_path.as_str()));
                g.set_has_library_folders(true);
                g.set_has_playable_files(has_playable);
                g.set_can_go_back(can_go_back);
                g.set_error_message(SharedString::from(""));
                g.set_loading(false);
                *browse_ui.last_files.lock() = files;
            });
        }
        Err(e) => {
            *browse_ui.last_files.lock() = Vec::new();
            let msg = e.to_string();
            let can_go_back = !browse_ui.history.lock().is_empty();
            let path_for_ui = path;
            // Build breadcrumbs *before* the UI hop so the closure
            // doesn't need to borrow `library_folders` across the
            // 'static event-loop boundary.
            let breadcrumbs = build_breadcrumbs(&path_for_ui, &library_folders);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let g = ui.global::<Browse>();
                replace_folder_model(&g, Vec::new());
                replace_rows_model(&g, Vec::new());
                // Keep breadcrumbs for the path we tried — gives the user
                // a way back up the tree even when the leaf is gone.
                replace_breadcrumb_model(&g, breadcrumbs);
                reset_selection(&g);
                g.set_current_path(SharedString::from(path_for_ui.as_str()));
                g.set_has_library_folders(true);
                g.set_has_playable_files(false);
                g.set_can_go_back(can_go_back);
                g.set_error_message(SharedString::from(msg));
                g.set_loading(false);
            });
        }
    }

    Ok(())
}

/// Re-sort the cached `last_files` to the current `BrowseUi` sort state
/// and rebuild the `Browse.rows` model in place. No DB hit. Runs on the
/// UI thread (called directly from the `request-sort` callback).
/// Selection is preserved — track ids are stable across a re-sort, only
/// the row order changes.
pub fn resort_and_apply(ui: &AppWindow, browse_ui: &Arc<BrowseUi>) {
    let sort_field = browse_ui.sort_field();
    let sort_dir = browse_ui.sort_dir();
    let ui_rows: Vec<UiTrackListRow> = {
        let mut files = browse_ui.last_files.lock();
        sort_browse_files(&mut files, &sort_field, &sort_dir);
        files.iter().map(to_slint_browse_track_row).collect()
    };
    let g = ui.global::<Browse>();
    replace_rows_model(&g, ui_rows);
    apply_selection_to_rows(&g);
}

/// Flip `is_favorite` on a single row in the Slint `VecModel`. Only
/// touches the affected row — scroll position and neighbours stay put.
/// Mirrors `tracks::apply_row_favorite`.
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let rows = ui.global::<Browse>().get_rows();
        let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
            return;
        };
        for i in 0..vm.row_count() {
            let Some(mut r) = vm.row_data(i) else {
                continue;
            };
            if i64::from(r.id) == id {
                r.is_favorite = fav;
                vm.set_row_data(i, r);
                break;
            }
        }
    });
}

/// Set `rating` on a single row in the Slint `VecModel` — the rating analogue
/// of [`apply_row_favorite`]. Only touches the affected row.
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let rows = ui.global::<Browse>().get_rows();
        let Some(vm) = rows.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
            return;
        };
        for i in 0..vm.row_count() {
            let Some(mut r) = vm.row_data(i) else {
                continue;
            };
            if i64::from(r.id) == id {
                r.rating = rating;
                vm.set_row_data(i, r);
                break;
            }
        }
    });
}
