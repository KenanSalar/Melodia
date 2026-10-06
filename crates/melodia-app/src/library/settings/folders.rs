//! Folder-management API: add / remove / list / watch the library's
//! source directories. Lives alongside the settings setters because
//! folders are settings-adjacent (the `Folder` table backs
//! `Settings → Library → Music Folders`) and shares the same
//! validation-then-persist cadence. Scanning one is `library::scan`'s.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::library::scan;
use crate::services;
use crate::state::AppState;
use crate::tasks::{self, TaskSpawner};
use melodia_core::entities::folder;
use melodia_core::error::{AppError, describe};
use melodia_store::database::{DbPool, queries};
use melodia_store::media::ingest::watcher::FolderWatcher;

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
    retarget_watcher(state).await;

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

    queries::folder::delete_folders(db, &children_to_remove).await?;

    let canonical = melodia_core::utils::canonicalize_path(new_path)
        .map_err(|e| AppError::Validation(format!("Cannot resolve path: {e}")))?;
    let canonical_str = canonical.to_string_lossy().into_owned();

    queries::folder::insert_folder(db, &canonical_str, true).await
}

pub async fn remove_folder(state: &AppState, id: i64) -> Result<(), AppError> {
    // Read ahead of the delete: the covers it releases are the ones named here and not after it.
    let referenced_before = queries::artwork::referenced_filenames(&state.db).await?;
    queries::folder::delete_folders(&state.db, &[id]).await?;
    // Cascade-delete removes every track in this folder; subscribers (Tracks
    // view + folder list) need to re-fetch or the UI keeps the stale rows.
    state.library_changed.bump();
    tasks::artwork_sweep::retire_released(
        &TaskSpawner::from_state(state),
        state,
        referenced_before,
    );
    retarget_watcher(state).await;
    Ok(())
}

pub async fn get_folders(state: &AppState) -> Result<Vec<folder::Folder>, AppError> {
    queries::folder::get_all_folders(&state.db).await
}

/// The platform's own Music folder, when adding it would neither be refused nor replace a folder
/// already in the library, for the welcome card to offer in one click.
///
/// Replacing is ruled out as well as refusing: a parent supersedes its children by deleting their
/// tracks, and a click on an offer is no place to spend a library's play counts and favourites.
pub async fn suggested_music_folder(state: &AppState) -> Result<Option<PathBuf>, AppError> {
    let Some(music_dir) = dirs::audio_dir() else {
        return Ok(None);
    };
    let existing = queries::folder::get_all_folders(&state.db).await?;
    let addable =
        validate_folder_path(&music_dir, &existing).is_ok_and(|children| children.is_empty());
    Ok(addable.then_some(music_dir))
}

pub async fn toggle_folder_watching(state: &AppState, enabled: bool) -> Result<(), AppError> {
    if enabled {
        start_watcher(state).await?;
        // Catch files added / removed while the watcher was off — the
        // watcher itself only reports live events.
        scan::reconcile_watched_folders(state);
    } else {
        with_watcher(state, FolderWatcher::stop).await?;
    }
    Ok(())
}

/// Starts the watcher over every enabled folder, replacing whatever it watched before.
pub(crate) async fn start_watcher(state: &AppState) -> Result<(), AppError> {
    let paths = enabled_folder_paths(state).await?;
    with_watcher(state, move |watcher| watcher.start(&paths)).await?
}

/// Points a running watcher at the folder list as it now stands, so a folder added in Settings
/// is watched from now rather than from the next launch. A failure costs only that, so it is
/// logged rather than failing the add or remove it follows.
async fn retarget_watcher(state: &AppState) {
    let retarget = async {
        let paths = enabled_folder_paths(state).await?;
        with_watcher(state, move |watcher| watcher.retarget(&paths)).await
    };
    if let Err(e) = retarget.await {
        log::warn!("Folder watcher didn't follow the folder list: {}", describe(&e));
    }
}

async fn enabled_folder_paths(state: &AppState) -> Result<Vec<PathBuf>, AppError> {
    let folders = queries::folder::get_all_folders(&state.db).await?;
    Ok(folders.iter().filter(|f| f.is_enabled).map(|f| PathBuf::from(&f.path)).collect())
}

/// Runs `op` against the watcher on the blocking pool: registering a recursive watch walks the
/// whole tree under the lock, and Add Folder's flow runs on the UI thread.
async fn with_watcher<R: Send + 'static>(
    state: &AppState,
    op: impl FnOnce(&mut FolderWatcher) -> R + Send + 'static,
) -> Result<R, AppError> {
    let watcher = Arc::clone(&state.watcher);
    tokio::task::spawn_blocking(move || op(&mut watcher.lock()))
        .await
        .map_err(|e| AppError::watcher("Folder watcher task failed", e))
}

/// Persist the `folder_watching_enabled` flag *first*, then flip the watcher.
/// Persist-first means a `start()` failure leaves disk consistent with the
/// user's intent — `tasks::resume_watching::run` will retry on next launch
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
