//! Holds the browsed-logo cache to its bounds, and retires the files that fall out of it.
//!
//! **Prune, then sweep, in that order only.** `library::radio` drops the answers no longer worth
//! keeping, which stops their files being referenced, and [`super::artwork_sweep`] retires what no
//! column names. Reversed, every dropped row's file still counts as referenced and the store never
//! shrinks.
//!
//! **Run on a Radio leave rather than after a scan**: the store grows only while the page is open,
//! and a user who only browses radio may not scan for weeks.

use crate::library;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, describe};
use melodia_store::database::DbPool;

/// Detached, the caller being a section leave, and tracked, so a shutdown waits for the unlinks.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let state = state.clone();
    spawner.spawn(async move {
        if let Err(e) = run(&state.db, &state.paths).await {
            log::warn!("Radio logo cache pass failed: {}", describe(&e));
        }
    });
}

async fn run(db: &DbPool, paths: &Paths) -> Result<(), AppError> {
    let dropped = library::radio::prune_logo_answers(db).await?;
    if dropped > 0 {
        log::debug!("radio: {dropped} cached logo answer(s) fell outside the cache bounds");
    }
    super::artwork_sweep::run_radio_logos(db, paths).await
}

#[cfg(test)]
#[path = "tests/radio_logo_cache_tests.rs"]
mod tests;
