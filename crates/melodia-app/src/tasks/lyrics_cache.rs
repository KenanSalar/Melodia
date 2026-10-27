//! Holds the lyrics store to its bounds, **run when the store stops growing rather than after a
//! scan**: only a mounted lyrics panel calls `library::lyrics::for_track`, so the last one closing,
//! or the switch going off, is exactly when it stops.

use std::sync::Arc;

use crate::library;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, describe};

/// Detached, the caller being a view closing, and tracked, so a shutdown waits for the unlinks.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let paths = Arc::clone(&state.paths);
    spawner.spawn(async move {
        match run(paths).await {
            Ok(0) => {}
            Ok(count) => log::debug!("lyrics: {count} cached answer(s) retired"),
            Err(e) => log::warn!("Lyrics cache pass failed: {}", describe(&e)),
        }
    });
}

/// Split from `spawn` so the count and the failure are observable. A `JoinError` reads as a failed
/// pass, the caller treating the two alike.
async fn run(paths: Arc<Paths>) -> Result<u32, AppError> {
    tokio::task::spawn_blocking(move || library::lyrics::prune_store(&paths))
        .await
        .map_err(AppError::io_source)?
}

#[cfg(test)]
#[path = "tests/lyrics_cache_tests.rs"]
mod tests;
