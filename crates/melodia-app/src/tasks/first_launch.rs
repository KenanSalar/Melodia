use std::path::Path;

use crate::library;
use crate::library::scan::ScanOutcome;
use crate::services;
use crate::state::AppState;
use melodia_core::entities::folder::Folder;
use melodia_core::error::{AppResult, describe};
use melodia_store::database::queries;

/// Translate of `startup::run_async_init` from the Tauri version.
///
/// Two responsibilities, both gated on persisted settings:
///   1. On first launch, auto-add `dirs::audio_dir()` (e.g. `~/Music`) as a
///      watched library folder, then kick off an initial scan in the background.
///   2. If `folder_watching_enabled` was previously true, restart the
///      `FolderWatcher` with the persisted folder paths so file events resume
///      flowing into the `file_event_processor`.
pub async fn run(state: &AppState) -> AppResult<()> {
    let settings = services::settings::read_settings(&state.paths).unwrap_or_else(|e| {
        log::warn!("Failed to read settings during first-launch init: {}", describe(&e));
        services::settings::SettingsData::default()
    });

    // Set when the user cancels the auto-scan, so the reconcile below doesn't start it over.
    let mut auto_scan_stopped = false;
    if !settings.library.music_folder_auto_added {
        if let Some(music_dir) = dirs::audio_dir()
            && music_dir.exists()
            && let Ok(canonical) = melodia_core::utils::canonicalize_path(&music_dir)
        {
            let path = canonical.to_string_lossy().into_owned();
            let existing = queries::folder::get_all_folders(&state.db).await.unwrap_or_default();
            if !already_watched(&existing, &canonical) {
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
                        match library::scan::scan_folder(state, folder.id).await {
                            Ok(outcome) => auto_scan_stopped = outcome == ScanOutcome::Stopped,
                            Err(e) => {
                                log::warn!("Auto-scan of Music folder failed: {}", describe(&e));
                            }
                        }
                    }
                    Err(e) => log::warn!("Failed to auto-add Music folder: {}", describe(&e)),
                }
            }
        }

        // Not the snapshot above: the scan can run for minutes, and anything the user changed
        // meanwhile would be written back over.
        let marked = services::settings::mutate_settings(&state.paths, |settings| {
            settings.library.music_folder_auto_added = true;
        });
        if let Err(e) = marked {
            log::warn!("Failed to save music_folder_auto_added flag: {}", describe(&e));
        }
    }

    if settings.library.folder_watching_enabled {
        if let Err(e) = library::settings::folders::start_watcher(state).await {
            log::warn!("Failed to start folder watcher: {}", describe(&e));
        }
        // Catch files added / removed since the previous session —
        // the watcher only reports live events from now on.
        if !auto_scan_stopped {
            library::scan::reconcile_watched_folders(state);
        }
    }

    Ok(())
}

/// Whether `candidate` is one of the folders already in the library.
///
/// Both sides are canonicalized, because the two spellings arrive from different places and are
/// only ever equal by accident: `dirs::audio_dir()` builds one from `$HOME`, and the stored side
/// is whatever the user picked in a file dialog. A trailing separator, a symlinked home, or a
/// case difference on a case-insensitive volume each make the same directory read as new, and the
/// cost is the whole music library indexed and listed twice.
///
/// A row whose path no longer resolves cannot be the candidate, which does: the auto-add runs
/// behind a `music_dir.exists()`.
fn already_watched(existing: &[Folder], candidate: &Path) -> bool {
    existing.iter().any(|folder| {
        melodia_core::utils::canonicalize_path(Path::new(&folder.path))
            .is_ok_and(|resolved| resolved == candidate)
    })
}

#[cfg(test)]
#[path = "tests/first_launch_tests.rs"]
mod tests;
