use std::path::{Path, PathBuf};

use crate::database::queries;
use crate::error::AppResult;
use crate::library;
use crate::services;
use crate::state::AppState;

/// Translate of `startup::run_async_init` from the Tauri version.
///
/// Two responsibilities, both gated on persisted settings:
///   1. On first launch, auto-add `dirs::audio_dir()` (e.g. `~/Music`) as a
///      watched library folder, then kick off an initial scan in the background.
///   2. If `folder_watching_enabled` was previously true, restart the
///      `FolderWatcher` with the persisted folder paths so file events resume
///      flowing into the `file_event_processor`.
pub async fn run(state: &AppState) -> AppResult<()> {
    let mut settings = services::settings::read_settings(&state.paths).unwrap_or_else(|e| {
        log::warn!("Failed to read settings during first-launch init: {e}");
        services::settings::SettingsData::default()
    });

    if !settings.library.music_folder_auto_added {
        if let Some(music_dir) = dirs::audio_dir()
            && music_dir.exists()
            && let Ok(canonical) = crate::utils::canonicalize_path(&music_dir)
        {
            let path = canonical.to_string_lossy().into_owned();
            let existing = queries::folder::get_all_folders(&state.db).await.unwrap_or_default();
            let dup = existing.iter().any(|f| {
                crate::utils::canonicalize_path(Path::new(&f.path)).is_ok_and(|p| p == canonical)
            });
            if !dup {
                match queries::folder::insert_folder(&state.db, &path, true).await {
                    Ok(folder) => {
                        log::info!("Auto-added Music folder: {path}");
                        // Inline-await so the watcher start below sees a
                        // committed DB state. Otherwise both the in-flight
                        // scan and the watcher's create-events race to
                        // ingest the same files through the single writer
                        // connection. `run()` is itself a tracked task in
                        // `main.rs`, so the outer task_tracker is what
                        // covers shutdown.
                        if let Err(e) =
                            library::settings::scan_folder_internal(state, folder.id).await
                        {
                            log::warn!("Auto-scan of Music folder failed: {e}");
                        }
                    }
                    Err(e) => log::warn!("Failed to auto-add Music folder: {e}"),
                }
            }
        }

        settings.library.music_folder_auto_added = true;
        if let Err(e) = services::settings::write_settings(&state.paths, &settings) {
            log::warn!("Failed to save music_folder_auto_added flag: {e}");
        }
    }

    if settings.library.folder_watching_enabled {
        match queries::folder::get_all_folders(&state.db).await {
            Ok(folders) => {
                let paths: Vec<PathBuf> = folders
                    .iter()
                    .filter(|f| f.is_enabled)
                    .map(|f| PathBuf::from(&f.path))
                    .collect();
                {
                    let mut watcher = state.watcher.lock();
                    if let Err(e) = watcher.start(&paths) {
                        log::warn!("Failed to start folder watcher: {e}");
                    }
                }
                // Catch files added / removed since the previous session —
                // the watcher only reports live events from now on.
                library::settings::reconcile_watched_folders(state);
            }
            Err(e) => log::warn!("Failed to load folders for watcher: {e}"),
        }
    }

    Ok(())
}
