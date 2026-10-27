//! The folder fetch, the in-memory re-sort and the row patches.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::{ComponentHandle, SharedString, Weak};

use super::breadcrumbs::{build_breadcrumbs, folder_basename, sort_browse_files};
use super::cards::{self, BrowseViewMode};
use super::models::{replace_breadcrumb_model, replace_folder_model, replace_rows_model};
use super::selection::{apply_selection_to_rows, reset_selection};
use super::{BrowseUi, to_slint_browse_track_rows};
use crate::ui::model_patch;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::browse::{BrowseFile, BrowseFolder, BrowseResult};
use melodia_core::entities::folder::Folder;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{
    AppWindow, BreadcrumbRow as UiBreadcrumbRow, Browse, BrowseFolderRow as UiBrowseFolderRow,
    TrackListRow as UiTrackListRow,
};

/// Fetches `path`, the root or a folder, and paints it unless a later fetch overtook this one.
/// Clears the selection, whose ids the new folder needn't hold.
pub async fn fetch_and_apply(
    state: &AppState,
    browse_ui: &Arc<BrowseUi>,
    weak: Weak<AppWindow>,
    path: String,
) -> AppResult<()> {
    let my_token = begin_fetch(browse_ui, &weak);

    if path.is_empty() {
        return apply_root(state, browse_ui, &weak, my_token).await;
    }

    // A failure paints the error state rather than propagating, so a passing filesystem error
    // can't stop the watcher's re-fetches. The folder list is read once, for both
    // `browse_directory` and the breadcrumbs.
    let library_folders = library::settings::get_folders(&state.db).await.unwrap_or_default();
    let result = library::browse::browse_directory(&state.db, path.clone(), &library_folders).await;
    if superseded(browse_ui, my_token) {
        return Ok(());
    }

    match result {
        Ok(res) => apply_folder(browse_ui, &weak, my_token, res, &library_folders).await,
        Err(e) => paint_listing(
            &weak,
            browse_ui,
            Listing {
                files: Vec::new(),
                folders: Vec::new(),
                // Still a way back up the tree when the leaf is gone.
                breadcrumbs: build_breadcrumbs(&path, &library_folders),
                current_path: path,
                has_library_folders: true,
                can_go_back: !browse_ui.history.lock().is_empty(),
                error_message: describe(&e),
            },
        ),
    }
    Ok(())
}

/// Bumps `fetch_token`, returning this fetch's ticket, and raises `loading`, which is harmless
/// even if a later fetch overtakes this one.
fn begin_fetch(browse_ui: &BrowseUi, weak: &Weak<AppWindow>) -> u64 {
    let my_token = browse_ui.fetch_token.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = weak.upgrade_in_event_loop(|ui| {
        ui.global::<Browse>().set_loading(true);
    });
    my_token
}

/// Whether a later fetch has overtaken the one holding `my_token`.
fn superseded(browse_ui: &BrowseUi, my_token: u64) -> bool {
    browse_ui.fetch_token.load(Ordering::Relaxed) != my_token
}

/// The root: the library folders as drillable rows.
async fn apply_root(
    state: &AppState,
    browse_ui: &Arc<BrowseUi>,
    weak: &Weak<AppWindow>,
    my_token: u64,
) -> AppResult<()> {
    let folders = library::settings::get_folders(&state.db).await?;
    if superseded(browse_ui, my_token) {
        return Ok(());
    }
    let browse_folders: Vec<BrowseFolder> = folders
        .iter()
        .filter(|f| f.is_enabled)
        .map(|f| BrowseFolder { name: folder_basename(&f.path), path: f.path.clone() })
        .collect();
    paint_listing(
        weak,
        browse_ui,
        Listing {
            files: Vec::new(),
            has_library_folders: !browse_folders.is_empty(),
            folders: browse_folders,
            breadcrumbs: Vec::new(),
            current_path: String::new(),
            can_go_back: false,
            error_message: String::new(),
        },
    );
    Ok(())
}

/// A drilled-in folder: warms both cover tiers around the sort, checking after each slow step
/// that no later fetch has overtaken this one.
async fn apply_folder(
    browse_ui: &Arc<BrowseUi>,
    weak: &Weak<AppWindow>,
    my_token: u64,
    res: BrowseResult,
    library_folders: &[Folder],
) {
    prewarm_row_tier(browse_ui, &res.files).await;
    if superseded(browse_ui, my_token) {
        return;
    }

    let mut files = res.files;
    sort_browse_files(&mut files, &browse_ui.sort_field(), &browse_ui.sort_dir());
    warm_card_tier(browse_ui, &files).await;
    if superseded(browse_ui, my_token) {
        return;
    }

    paint_listing(
        weak,
        browse_ui,
        Listing {
            files,
            folders: res.folders,
            breadcrumbs: build_breadcrumbs(&res.path, library_folders),
            current_path: res.path,
            has_library_folders: true,
            can_go_back: !browse_ui.history.lock().is_empty(),
            error_message: String::new(),
        },
    );
}

/// Walked in fetch order, ahead of the staleness check the sort follows, so a folder holding more
/// covers than the tier keeps a prefix that isn't the first to paint. A folder that big is outside
/// what Browse is for.
async fn prewarm_row_tier(browse_ui: &BrowseUi, files: &[BrowseFile]) {
    let unique_paths: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        files.iter().map(|f| f.row.artwork_path.as_deref()),
        browse_ui.cover_thumbs.capacity(),
    );
    if unique_paths.is_empty() {
        return;
    }
    let thumbs = browse_ui.cover_thumbs.clone();
    let _ = tokio::task::spawn_blocking(move || {
        thumbs.prewarm(&unique_paths);
    })
    .await;
}

/// The card tier, warmed after the sort: capped at a screenful, its prefix has to be the one that
/// paints. Awaited before the rows land, so that screenful is a cache hit.
async fn warm_card_tier(browse_ui: &Arc<BrowseUi>, files: &[BrowseFile]) {
    if browse_ui.view_mode() != BrowseViewMode::Card {
        return;
    }
    let unique = cards::first_screenful_paths(files);
    crate::ui::grid_prewarm::prewarm_off_thread(browse_ui, move |bu| {
        bu.warm_card_tier(&unique);
    })
    .await;
}

/// Everything one fetch paints, root, folder and error alike.
struct Listing {
    files: Vec<BrowseFile>,
    folders: Vec<BrowseFolder>,
    breadcrumbs: Vec<UiBreadcrumbRow>,
    current_path: String,
    has_library_folders: bool,
    can_go_back: bool,
    error_message: String,
}

/// Writes `listing` into the `Browse` global, and beside it the caches the card view rebuilds
/// from.
fn paint_listing(weak: &Weak<AppWindow>, browse_ui: &Arc<BrowseUi>, listing: Listing) {
    let browse_ui = browse_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let Listing {
            files,
            folders,
            breadcrumbs,
            current_path,
            has_library_folders,
            can_go_back,
            error_message,
        } = listing;
        let g = ui.global::<Browse>();
        // The rows are built from `&files` before `files` moves into the cache, so nothing is
        // cloned.
        replace_folder_model(&g, to_ui_folder_rows(&folders));
        replace_rows_model(&g, to_slint_browse_track_rows(&files));
        replace_breadcrumb_model(&g, breadcrumbs);
        reset_selection(&g);
        g.set_current_path(SharedString::from(current_path));
        g.set_has_library_folders(has_library_folders);
        g.set_can_go_back(can_go_back);
        g.set_error_message(SharedString::from(error_message));
        g.set_loading(false);
        *browse_ui.last_files.lock() = files;
        *browse_ui.last_folders.lock() = folders;
        cards::rebuild_cards(&ui, &browse_ui);
    });
}

/// The list view's folder rows, derived from the same `BrowseFolder`s the card view caches, so
/// the two can't drift.
fn to_ui_folder_rows(folders: &[BrowseFolder]) -> Vec<UiBrowseFolderRow> {
    folders
        .iter()
        .map(|f| UiBrowseFolderRow {
            name: SharedString::from(f.name.as_str()),
            path: SharedString::from(f.path.as_str()),
        })
        .collect()
}

/// Re-sorts the cached files and rebuilds `Browse.rows`, keeping the selection, ids being stable
/// across a re-sort. The cards need no write: a sort comes from a header that exists only while
/// the list is mounted, and the toggle rebuilds them from these same files.
pub fn resort_and_apply(ui: &AppWindow, browse_ui: &Arc<BrowseUi>) {
    let sort_field = browse_ui.sort_field();
    let sort_dir = browse_ui.sort_dir();
    let ui_rows: Vec<UiTrackListRow> = {
        let mut files = browse_ui.last_files.lock();
        sort_browse_files(&mut files, &sort_field, &sort_dir);
        to_slint_browse_track_rows(&files)
    };
    let g = ui.global::<Browse>();
    replace_rows_model(&g, ui_rows);
    apply_selection_to_rows(&g);
}

/// Patches the one row on screen, so the scroll holds.
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Browse>().get_rows(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// [`apply_row_favorite`]'s star-rating twin.
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Browse>().get_rows(), id, |r| {
            r.rating = rating;
        });
    });
}
