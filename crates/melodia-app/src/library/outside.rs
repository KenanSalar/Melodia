//! The files opened from outside every library folder, held to their rule once a launch
//! (`queries::outside`).

use std::collections::HashSet;
use std::path::Path;

use crate::library::settings::folders::folders_inside;
use crate::state::AppState;
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;
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
/// folder's scan to absorb it, found by the absorb's own canonical rule, a stat apiece.
fn outside_folders(folders: &[Folder]) -> Vec<i64> {
    let awaiting_absorb: HashSet<i64> = folders
        .iter()
        .filter(|f| f.is_enabled)
        .flat_map(|library| folders_inside(Path::new(&library.path), folders))
        .map(|nested| nested.id)
        .collect();
    folders
        .iter()
        .filter(|f| !f.is_enabled && !awaiting_absorb.contains(&f.id))
        .map(|f| f.id)
        .collect()
}
