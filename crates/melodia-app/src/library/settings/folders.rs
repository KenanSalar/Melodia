//! Folder-management API: add / remove / list / watch the library's
//! source directories. Lives alongside the settings setters because
//! folders are settings-adjacent (the `Folder` table backs
//! `Settings → Library → Music Folders`) and shares the same
//! validation-then-persist cadence. Scanning one is `library::scan`'s.

use std::path::{Path, PathBuf};

use crate::library::scan;
use crate::services;
use crate::state::AppState;
use melodia_core::entities::folder;
use melodia_core::error::AppError;
use melodia_store::database::{DbPool, queries};

/// Validates a new folder path against existing folders.
/// Returns IDs of existing child folders that should be removed (covered by the new parent).
fn validate_folder_path(
    new_path: &Path,
    existing_folders: &[folder::Folder],
) -> Result<Vec<i64>, AppError> {
    if !new_path.exists() {
        return Err(AppError::Validation(format!("Path does not exist: {}", new_path.display())));
    }
    if !new_path.is_dir() {
        return Err(AppError::Validation(format!(
            "Path is not a directory: {}",
            new_path.display()
        )));
    }

    let canonical_new = melodia_core::utils::canonicalize_path(new_path).map_err(|e| {
        AppError::Validation(format!("Cannot resolve path {}: {}", new_path.display(), e))
    })?;

    let mut children_to_remove = Vec::new();

    for folder in existing_folders {
        let existing_path = Path::new(&folder.path);
        let Ok(canonical_existing) = melodia_core::utils::canonicalize_path(existing_path) else {
            continue;
        };

        if canonical_new == canonical_existing {
            return Err(AppError::Validation(format!(
                "This folder is already in your library: {}",
                folder.path
            )));
        }

        if canonical_new.starts_with(&canonical_existing) {
            return Err(AppError::Validation(format!(
                "This folder is already covered by: {}",
                folder.path
            )));
        }

        if canonical_existing.starts_with(&canonical_new) {
            children_to_remove.push(folder.id);
        }
    }

    Ok(children_to_remove)
}

pub async fn add_folder(state: &AppState, path: String) -> Result<folder::Folder, AppError> {
    let folder = insert_replacing_children(&state.db, &path).await?;

    // Notify subscribers (Tracks view, folder list) — the new folder row is
    // visible immediately, and any child folders that were auto-aggregated
    // away cascade-deleted their tracks. The subsequent scan will fire its
    // own bump on completion.
    state.library_changed.bump();

    Ok(folder)
}

/// [`add_folder`]'s body, narrowed to the pool it reaches.
///
/// The delete is the half worth driving: a folder superseded by a new parent takes its tracks with
/// it, and a rescan brings the files back with none of the ratings, play counts or favourites that
/// were on them.
async fn insert_replacing_children(db: &DbPool, path: &str) -> Result<folder::Folder, AppError> {
    let new_path = Path::new(path);
    let existing_folders = queries::folder::get_all_folders(db).await?;
    let children_to_remove = validate_folder_path(new_path, &existing_folders)?;

    queries::folder::delete_folders_by_ids(db, &children_to_remove).await?;

    let canonical = melodia_core::utils::canonicalize_path(new_path)
        .map_err(|e| AppError::Validation(format!("Cannot resolve path: {e}")))?;
    let canonical_str = canonical.to_string_lossy().into_owned();

    queries::folder::insert_folder(db, &canonical_str, true).await
}

pub async fn remove_folder(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::folder::delete_folder(&state.db, id).await?;
    // Cascade-delete removes every track in this folder; subscribers (Tracks
    // view + folder list) need to re-fetch or the UI keeps the stale rows.
    state.library_changed.bump();
    Ok(())
}

pub async fn get_folders(state: &AppState) -> Result<Vec<folder::Folder>, AppError> {
    queries::folder::get_all_folders(&state.db).await
}

pub async fn toggle_folder_watching(state: &AppState, enabled: bool) -> Result<(), AppError> {
    if enabled {
        // Resolve folder paths via the async DB query *before* taking the
        // (sync) parking_lot lock — the guard must not span an await point.
        let folders = queries::folder::get_all_folders(&state.db).await?;
        let paths: Vec<PathBuf> =
            folders.iter().filter(|f| f.is_enabled).map(|f| PathBuf::from(&f.path)).collect();
        {
            let mut watcher = state.watcher.lock();
            watcher.start(&paths)?;
        }
        // Catch files added / removed while the watcher was off — the
        // watcher itself only reports live events.
        scan::reconcile_watched_folders(state);
    } else {
        let mut watcher = state.watcher.lock();
        watcher.stop();
    }
    Ok(())
}

/// Persist the `folder_watching_enabled` flag *first*, then flip the watcher.
/// Persist-first means a `start()` failure leaves disk consistent with the
/// user's intent — `tasks::first_launch::run` will retry on next launch
/// from the persisted flag. The reverse order would leave a running watcher
/// that doesn't restart next session if `mutate_settings` failed.
pub async fn set_folder_watching_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |s| {
        s.library.folder_watching_enabled = enabled;
    })?;
    toggle_folder_watching(state, enabled).await
}

#[cfg(test)]
#[path = "tests/folders_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/folders_writer_tests.rs"]
mod writer_tests;
