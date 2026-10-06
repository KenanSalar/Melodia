//! Restores cover art a cache found missing while the app runs.
//!
//! The boot scan already repairs whatever went missing while the app was closed. This is the
//! other half: a folder or file deleted mid-session, noticed only when a cover cache asks for it.
//! Reports arrive one per cover a cache tries, so each wake drains everything queued and asks
//! once; the reconcile it may start merges a second request into the one in flight.

use crate::library;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_core::utils::missing_artwork;

/// Reports drained per wake. Only bounds the buffer: every report says the same thing.
const DRAIN_BATCH: usize = 256;

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let Some(mut reports) = missing_artwork::install() else {
        return;
    };
    let state = state.clone();
    spawner.spawn_cancellable(|shutdown| async move {
        let mut drained = Vec::with_capacity(DRAIN_BATCH);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                received = reports.recv_many(&mut drained, DRAIN_BATCH) => {
                    if received == 0 {
                        break;
                    }
                    drained.clear();
                    if let Err(e) = library::scan::restore_missing_artwork(&state).await {
                        log::warn!("Restoring missing artwork failed: {}", describe(&e));
                    }
                }
            }
        }
    });
}
