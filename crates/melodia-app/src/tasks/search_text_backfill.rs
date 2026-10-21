//! Fills the folded search columns on the rows written before they existed: the tracks'
//! `search_text`, then the card grids' `search_name`.
//!
//! Every launch rather than once per install, `retroactive_hash`'s shape: an empty column is its
//! own marker, so the pass can't be told it finished by a settings file the database has since
//! been restored out from under, and a filled library costs one read of each empty index.

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_store::database::queries;

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let db = state.db.clone();
    spawner.spawn(async move {
        match queries::track::backfill_search_text(&db).await {
            Ok(0) => {}
            Ok(filled) => log::info!("Search text filled for {filled} tracks"),
            Err(e) => log::warn!("Search text backfill failed: {}", describe(&e)),
        }
        match queries::search_name::backfill_search_name(&db).await {
            Ok(0) => {}
            Ok(filled) => log::info!("Search names filled for {filled} grid rows"),
            Err(e) => log::warn!("Search name backfill failed: {}", describe(&e)),
        }
    });
}
