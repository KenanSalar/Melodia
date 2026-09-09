//! Holds the lyrics store to its bounds.
//!
//! **Run when Now Playing closes, not after a scan.** Nothing but that view calls
//! `library::lyrics::for_track`, so the store only grows while it is open and the close is exactly
//! when it stops: `tasks::radio_logo_cache`'s argument for pruning on a section leave, and for the
//! same reason the artwork sweep's scan trigger would be wrong here. A user who plays music for a
//! week without opening Now Playing writes nothing for this to collect.

use std::sync::Arc;

use crate::library;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, describe};

/// Prune the store in the background.
///
/// Detached rather than awaited: the caller is a view closing, and a directory listing is
/// maintenance nothing on screen is waiting for. Tracked, so a shutdown landing mid-pass waits for
/// the unlinks rather than tearing the runtime down under them.
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

/// The pass itself, on the blocking pool, answering with what it retired.
///
/// Narrowed off `spawn` so the count and the failure are both observable, as
/// [`super::radio_logo_cache`] narrows its own. A `JoinError` reads as a failed pass rather than as
/// its own arm: the caller does nothing different for a panicked worker than for a store it could
/// not read.
async fn run(paths: Arc<Paths>) -> Result<u32, AppError> {
    tokio::task::spawn_blocking(move || library::lyrics::prune_store(&paths))
        .await
        .map_err(AppError::io_source)?
}

#[cfg(test)]
#[path = "tests/lyrics_cache_tests.rs"]
mod tests;
