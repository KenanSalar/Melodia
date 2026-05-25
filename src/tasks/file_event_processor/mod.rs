//! Background task that drains the filesystem watcher's event channel,
//! deduplicates events within a 500 ms batching window, and applies the
//! resulting mutations to the database.
//!
//! Spawned once at startup from `main.rs`; exits on shutdown cancellation
//! after letting any in-flight batch's transaction commit so we don't
//! leave orphan rows behind.
//!
//! - [`dedup`] collapses a batch of raw watcher events to one event per path.
//! - [`reconcile`] extracts metadata and applies the batch to the database.

mod dedup;
mod reconcile;

use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use crate::media::watcher::FileEvent;
use crate::state::AppState;
use crate::tasks::TaskSpawner;

use dedup::deduplicate_events;
use reconcile::process_batch;

/// Spawn the file-event-processor on the shared task lifecycle so the main
/// shutdown sequence waits for the current batch's transaction to commit
/// before the runtime is torn down.
pub fn spawn(spawner: &TaskSpawner, state: &AppState, mut rx: mpsc::Receiver<FileEvent>) {
    let state = state.clone();

    spawner.spawn_cancellable(move |shutdown| async move {
        loop {
            let first = tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                event = rx.recv() => match event {
                    Some(e) => e,
                    None => break,
                },
            };

            let mut batch = vec![first];
            let deadline = Instant::now() + Duration::from_millis(500);
            while let Ok(Some(event)) = tokio::time::timeout_at(deadline, rx.recv()).await {
                batch.push(event);
            }

            let batch = deduplicate_events(batch);
            if batch.is_empty() {
                continue;
            }

            // RescanNeeded supersedes everything else in the batch; reconcile
            // every enabled folder against disk instead of trusting the
            // (now-truncated) per-file event stream. `deduplicate_events`
            // collapses any batch containing RescanNeeded to the singleton,
            // but check defensively rather than slice-pattern matching so a
            // future dedup change can't silently route a rescan into
            // `process_batch` (whose match arms `unreachable!()` on it).
            if batch.iter().any(|e| matches!(e, FileEvent::RescanNeeded)) {
                log::warn!("Rescan requested via watcher overflow flag");
                crate::library::settings::reconcile_watched_folders(&state);
                state
                    .rescan_notice_tx
                    .send_modify(|n| *n = n.wrapping_add(1));
                continue;
            }

            log::info!("Processing batch of {} file events", batch.len());

            // Don't bail on cancellation here — let the in-flight batch's tx
            // commit so we don't leave orphan rows behind.
            match process_batch(&state.db, &state.paths, &state.cover_cache, batch).await {
                Ok(()) => {
                    state.library_changed_tx.send_modify(|n| *n = n.wrapping_add(1));
                }
                Err(e) => log::error!("File event batch processing failed: {e}"),
            }
        }
        log::info!("File event processor stopped");
    });

    log::info!("File event processor started");
}

#[cfg(test)]
#[path = "../tests/file_event_processor_tests.rs"]
mod tests;
