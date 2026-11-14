//! Measures every library track carrying no gain tags in the background, while `ReplayGain` and
//! measuring are both on and the user hasn't stopped it this session. The files are never written.
//!
//! **Event-driven, never polled.** A pass runs at launch, then on a kick from the controls, a
//! library change or a scan letting go, and steps aside for a scan, which reads the same disks and
//! needs the one writer. Off or stopped, the loop waits on the kick alone, so the switch off costs
//! one parked task and no database work.

mod pass;
mod pool;

use std::collections::HashSet;
use std::sync::Arc;

use crate::state::{AppState, MeasureProgress};
use crate::tasks::TaskSpawner;
use pass::Controls;

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let mut kick = state.loudness.subscribe_kick();
    let mut library = state.library_changed.subscribe();
    let mut busy = state.scan.subscribe_busy();
    let loudness = state.loudness.clone();
    let scan = state.scan.clone();
    let db = state.db.clone();
    let playback = state.playback_ctx();

    spawner.spawn_cancellable(move |shutdown| async move {
        let controls =
            Arc::new(Controls { db, loudness, scan, playback, shutdown: shutdown.clone() });
        // Out of reach for this session, as on an unmounted drive, and tried again at the next
        // launch rather than recorded, which would keep the file from ever being measured.
        let mut out_of_reach = HashSet::new();
        loop {
            if controls.measuring() {
                if !controls.scan.is_busy() {
                    // A pass reads the library afresh, so what moved before it owes no second.
                    library.borrow_and_update();
                    busy.borrow_and_update();
                    let end = pass::run(&controls, &mut out_of_reach).await;
                    if let Some(progress) = end.progress() {
                        controls.loudness.publish(progress);
                    }
                }
            } else if !controls.loudness.is_stopped() {
                controls.loudness.publish(MeasureProgress::default());
            }

            let measuring = controls.measuring();
            let changed = tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                changed = kick.changed() => changed,
                changed = library.changed(), if measuring => changed,
                // Every move wakes it and the top asks whether a scan still runs: waiting for zero
                // would resolve at once while none does.
                changed = busy.changed(), if measuring => changed,
            };
            if changed.is_err() {
                break;
            }
        }
        log::info!("Loudness task stopped");
    });

    log::info!("Loudness task started");
}
