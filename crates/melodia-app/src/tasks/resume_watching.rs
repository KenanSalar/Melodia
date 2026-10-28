use crate::library;
use crate::services;
use crate::state::AppState;
use melodia_core::error::{AppResult, describe};

/// Restarts the watcher over the persisted folders and reconciles them against what changed while
/// the app was closed; with watching off, only finishes the imports a quit cut short. Adds no
/// folder of its own, a fresh install starting empty.
pub async fn run(state: &AppState) -> AppResult<()> {
    let settings = services::settings::read_settings(&state.paths).unwrap_or_else(|e| {
        log::warn!("Failed to read settings during startup init: {}", describe(&e));
        services::settings::SettingsData::default()
    });

    if settings.library.folder_watching_enabled {
        if let Err(e) = library::settings::folders::start_watcher(state).await {
            log::warn!("Failed to start folder watcher: {}", describe(&e));
        }
        library::scan::reconcile_watched_folders(state);
    } else {
        // Watching off opts out of following the disk, not out of an import the user asked for.
        library::scan::finish_interrupted_imports(state);
    }

    Ok(())
}
