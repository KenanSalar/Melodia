use crate::library;
use crate::services;
use crate::state::AppState;
use melodia_core::error::{AppResult, describe};

/// Restarts the `FolderWatcher` over the persisted folders when watching is on, then reconciles
/// each one against what changed on disk while the app was closed. With watching off it only
/// finishes the imports a quit cut short.
///
/// Adds no folder of its own: a fresh install starts empty, and the welcome card's first panel is
/// where the user picks one, the platform's Music folder among the offers.
pub async fn run(state: &AppState) -> AppResult<()> {
    let settings = services::settings::read_settings(&state.paths).unwrap_or_else(|e| {
        log::warn!("Failed to read settings during startup init: {}", describe(&e));
        services::settings::SettingsData::default()
    });

    if settings.library.folder_watching_enabled {
        if let Err(e) = library::settings::folders::start_watcher(state).await {
            log::warn!("Failed to start folder watcher: {}", describe(&e));
        }
        // Catch files added / removed since the previous session —
        // the watcher only reports live events from now on.
        library::scan::reconcile_watched_folders(state);
    } else {
        // Watching off opts out of following the disk, but an import a quit cut short is the
        // user's own request, half done.
        library::scan::finish_interrupted_imports(state);
    }

    Ok(())
}
