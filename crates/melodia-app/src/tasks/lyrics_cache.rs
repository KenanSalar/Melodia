//! Holds the lyrics store to its bounds.
//!
//! **Run when Now Playing closes, not after a scan.** Nothing but that view calls
//! `library::lyrics::for_track`, so the store only grows while it is open and the close is exactly
//! when it stops: `tasks::radio_logo_cache`'s argument for pruning on a section leave, and for the
//! same reason the artwork sweep's scan trigger would be wrong here. A user who plays music for a
//! week without opening Now Playing writes nothing for this to collect.

use crate::library;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;

/// Prune the store in the background.
///
/// Detached rather than awaited: the caller is a view closing, and a directory listing is
/// maintenance nothing on screen is waiting for. Tracked, so a shutdown landing mid-pass waits for
/// the unlinks rather than tearing the runtime down under them.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let state = state.clone();
    spawner.spawn(async move {
        let paths = state.paths.clone();
        let pruned =
            tokio::task::spawn_blocking(move || library::lyrics::prune_store(&paths)).await;

        match pruned {
            Ok(Ok(0)) => {}
            Ok(Ok(count)) => log::debug!("lyrics: {count} cached answer(s) retired"),
            Ok(Err(e)) => log::warn!("Lyrics cache pass failed: {}", describe(&e)),
            Err(e) => log::warn!("Lyrics cache pass did not finish: {e}"),
        }
    });
}
