//! Restores cover art a cache found missing: deleted mid-session, or, with watching off and so no
//! boot scan, while the app was closed. Reports arrive one per cover tried, so each wake drains
//! them all and asks once, and the reconcile it may start merges into one already in flight.

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
