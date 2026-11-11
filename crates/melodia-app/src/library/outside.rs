//! The files opened from outside every library folder, held to their rule once a launch
//! (`queries::outside`).

use std::path::Path;

use crate::state::AppState;
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;
use melodia_core::utils::canonicalize_path;
use melodia_engine::player::engine::state::lock_state;
use melodia_store::database::queries;

/// Sweeps the outside folders, keeping what a playlist or the restored queue holds. Runs after the
/// restore and before anything can import, so a row it deletes is one nothing holds.
pub async fn sweep(state: &AppState) -> Result<(), AppError> {
    let folders = queries::folder::get_all_folders(&state.db).await?;
    if folders.iter().all(|f| f.is_enabled) {
        return Ok(());
    }
    let outside = tokio::task::spawn_blocking(move || outside_folders(&folders))
        .await
        .map_err(|e| AppError::scanner("Outside folder task failed", e))?;
    let queue = lock_state(&state.player_state).queue.play_ids();
    queries::outside::sweep(&state.db, &outside, &queue).await
}

/// The disabled folders no library folder holds. One a library folder holds is waiting for that
/// folder's scan to absorb it.
fn outside_folders(folders: &[Folder]) -> Vec<i64> {
    let library: Vec<&Path> =
        folders.iter().filter(|f| f.is_enabled).map(|f| Path::new(&f.path)).collect();
    folders
        .iter()
        .filter(|f| !f.is_enabled && !awaits_absorb(Path::new(&f.path), &library))
        .map(|f| f.id)
        .collect()
}

/// Whether a library folder holds `dir`. The stored spelling answers first, so a nested folder
/// whose drive is offline isn't swept as outside, and only one spelled elsewhere costs a stat.
fn awaits_absorb(dir: &Path, library: &[&Path]) -> bool {
    let inside = |dir: &Path| library.iter().any(|root| dir.starts_with(root));
    inside(dir) || canonicalize_path(dir).is_ok_and(|canonical| inside(&canonical))
}
