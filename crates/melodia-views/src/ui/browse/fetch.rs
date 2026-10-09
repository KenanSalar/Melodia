//! Folder fetch + sort + favourite-row update. Bumps `fetch_token` to
//! drop stale UI writes when a faster navigation overtakes us.

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
use melodia_core::error::AppResult;
use melodia_ui::{
    AppWindow, BreadcrumbRow as UiBreadcrumbRow, Browse, BrowseFolderRow as UiBrowseFolderRow,
    TrackListRow as UiTrackListRow,
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
    let my_token = begin_fetch(browse_ui, &weak);

    if path.is_empty() {
        return apply_root(state, browse_ui, &weak, my_token).await;
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
                // Keep breadcrumbs for the path we tried — gives the user
                // a way back up the tree even when the leaf is gone.
                breadcrumbs: build_breadcrumbs(&path, &library_folders),
                current_path: path,
                has_library_folders: true,
                can_go_back: !browse_ui.history.lock().is_empty(),
                error_message: e.to_string(),
            },
        ),
    }
    Ok(())
}

/// Bump `fetch_token`, returning this fetch's ticket, and flip loading on (UI thread).
/// Tolerates a missed token-bump race; even if a later fetch overtakes us, painting
/// `loading: true` briefly is harmless.
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

/// Root view: render the library folder list as drillable rows.
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

/// A drilled-in folder: warm both cover tiers around the sort, checking between each slow step
/// that a later fetch hasn't overtaken this one.
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

    // Sort to the user's current order. The sorted list is cached
    // in `last_files` (callbacks like `play-row` / selection read
    // it without round-tripping) — but the caching happens inside
    // the UI closure below as a *move*, not a `clone_from`.
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

/// Prewarm cover thumbnails. Walked in *fetch* order — the sort
/// hasn't run yet — so on a folder holding more unique
/// covers than the tier, the surviving prefix isn't the one that
/// paints first. Moving the prewarm past the sort would put it
/// after the staleness check it currently precedes, and a folder
/// that deep is well outside what Browse is for.
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

/// The card tier, warmed **after** the sort — unlike the row-tier
/// prewarm, this one is capped at a screenful, so the prefix
/// that survives the cap has to be the prefix that paints. Awaited
/// before the rows land (the Albums prewarm-then-write ordering), so
/// the first screenful of cards is a cache hit.
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

/// Write `listing` into the `Browse` global on the UI thread, and the caches the card view
/// rebuilds from beside it.
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
        // Build the rows from `&files`, then move `files` itself
        // into the `last_files` cache as the final step — one
        // move, no clone. Covers resolve lazily per visible row via
        // `RowCovers.request`.
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

/// Project the cached folder list into the Slint rows the list view draws.
/// Both fetch arms build their `Vec<BrowseFolder>` first and derive these from
/// it, so the folder list has one source and the card view's cache can't drift
/// from what the list is showing.
fn to_ui_folder_rows(folders: &[BrowseFolder]) -> Vec<UiBrowseFolderRow> {
    folders
        .iter()
        .map(|f| UiBrowseFolderRow {
            name: SharedString::from(f.name.as_str()),
            path: SharedString::from(f.path.as_str()),
        })
        .collect()
}

/// Re-sort the cached `last_files` to the current `BrowseUi` sort state
/// and rebuild the `Browse.rows` model in place. No DB hit. Runs on the
/// UI thread (called directly from the `request-sort` callback).
/// Selection is preserved — track ids are stable across a re-sort, only
/// the row order changes.
///
/// The card model needs no write here: `request-sort` comes from a `TrackList`
/// column header, which only exists while the list is mounted, and the toggle
/// rebuilds the cards from this same re-ordered `last_files`.
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

/// Flip `is_favorite` on a single row in the Slint `VecModel`. Only
/// touches the affected row — scroll position and neighbours stay put.
/// Mirrors `tracks::apply_row_favorite`.
pub fn apply_row_favorite(weak: &Weak<AppWindow>, id: i64, fav: bool) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Browse>().get_rows(), id, |r| {
            r.is_favorite = fav;
        });
    });
}

/// Set `rating` on a single row in the Slint `VecModel` — the rating analogue
/// of [`apply_row_favorite`].
pub fn apply_row_rating(weak: &Weak<AppWindow>, id: i64, rating: i32) {
    let _ = weak.upgrade_in_event_loop(move |ui| {
        model_patch::patch_track_row_by_id(&ui.global::<Browse>().get_rows(), id, |r| {
            r.rating = rating;
        });
    });
}
