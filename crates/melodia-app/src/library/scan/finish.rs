//! What follows a scan that completed.

use crate::library::settings::folders::NestedFolder;
use crate::services;
use crate::state::AppState;
use crate::tasks::{self, TaskSpawner};
use melodia_core::entities::folder::Folder;
use melodia_core::error::AppError;
use melodia_store::database::queries;

/// Hands `folder` the folders nested in it, tracks and all, now that a scan of it has completed.
pub(super) async fn absorb_nested(
    state: &AppState,
    folder: &Folder,
    nested: &[NestedFolder],
) -> Result<(), AppError> {
    if nested.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = nested.iter().map(|f| f.id).collect();
    queries::folder::absorb_folders(&state.db, folder.id, &ids).await?;
    log::info!("Folded {} nested folder(s) into {}", ids.len(), folder.path);
    Ok(())
}

/// Stamps the folder as scanned, which is also how a launch tells a finished import from one a quit
/// cut short, and starts the passes a completed scan feeds. A stopped scan owes neither: the folder
/// isn't scanned, and the next scan of it runs them.
pub(super) async fn after_completed(state: &AppState, folder_id: i64) -> Result<(), AppError> {
    let now = melodia_core::utils::now_rfc3339();
    queries::folder::update_folder_last_scanned(&state.db, folder_id, &now).await?;

    services::artist_images::spawn_fetch(
        state.paths.clone(),
        state.db.clone(),
        state.http_client().clone(),
    );
    let spawner = TaskSpawner::from_state(state);
    tasks::retroactive_hash::spawn(&spawner, state);
    // After the orphan pass committed, so the rows it deleted are already gone from the
    // reference set this reads.
    tasks::artwork_sweep::spawn(&spawner, state);
    Ok(())
}
