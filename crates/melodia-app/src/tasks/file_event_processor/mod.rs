//! Drains the watcher's events in 500 ms batches, deduplicates them and applies them to the
//! database. On shutdown an in-flight batch's transaction commits rather than leave orphan rows.

mod dedup;
mod reconcile;

use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::media::ingest::watcher::FileEvent;

use dedup::deduplicate_events;
use reconcile::process_batch;

/// Drops the echo of our own tag writes, which lands as a rename: `atomic_file::rewrite_with_sync`
/// renames an edited copy over the track from a temp with no audio extension, so `classify_event`
/// calls it a `Created`, or a `Modified` on backends reporting it so. A real `Renamed` is never
/// ours. Left in, the echo costs a re-hash, a tag parse and an artwork extract.
///
/// `take_recent` consumes, so a path is suppressed at most once per write ([`SelfWrites`] has the
/// trades). `retain` visits each event once, in order, and rebuilds nothing when nothing is ours.
fn suppress_self_writes(batch: &mut Vec<FileEvent>, self_writes: &SelfWrites) {
    batch.retain(|event| match event {
        FileEvent::Created(path) | FileEvent::Modified(path) => !self_writes.take_recent(path),
        _ => true,
    });
}

/// What a drained batch amounts to, once the rescan flag and our own writes are accounted for.
enum BatchPlan {
    /// Nothing left worth a database round trip.
    Nothing,
    /// The watcher overflowed, so its per-file stream is truncated and untrustworthy.
    Rescan,
    /// Apply these, the echo of our own tag writes already dropped.
    Process(Vec<FileEvent>),
}

/// The `any` makes this correct for an undeduplicated batch too, since `process_batch`
/// `unreachable!()`s on a `RescanNeeded`.
///
/// The rescan is answered *before* the suppression because `take_recent` consumes: spending a mark
/// on a batch discarded for a full reconcile would leave the real echo unsuppressed inside its TTL.
/// [`SelfWrites`]' header states that ordering as settled and cannot see this function.
fn plan_batch(mut batch: Vec<FileEvent>, self_writes: &SelfWrites) -> BatchPlan {
    if batch.is_empty() {
        return BatchPlan::Nothing;
    }
    if batch.iter().any(|e| matches!(e, FileEvent::RescanNeeded)) {
        return BatchPlan::Rescan;
    }

    let before = batch.len();
    suppress_self_writes(&mut batch, self_writes);
    let suppressed = before - batch.len();
    if suppressed > 0 {
        log::debug!("Suppressed {suppressed} watcher event(s) from our own tag writes");
    }

    if batch.is_empty() { BatchPlan::Nothing } else { BatchPlan::Process(batch) }
}

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

            match plan_batch(deduplicate_events(batch), &state.self_writes) {
                BatchPlan::Nothing => {}
                BatchPlan::Rescan => {
                    log::warn!("Rescan requested via watcher overflow flag");
                    crate::library::scan::reconcile_watched_folders(&state);
                    state.rescan_notice.bump();
                }
                BatchPlan::Process(batch) => {
                    log::info!("Processing batch of {} file events", batch.len());

                    // Not cancelled: the in-flight batch's transaction commits.
                    match process_batch(&state.db, &state.paths, &state.cover_cache, batch).await {
                        Ok(()) => state.library_changed.bump(),
                        Err(e) => {
                            log::error!("File event batch processing failed: {}", describe(&e));
                        }
                    }
                }
            }
        }
        log::info!("File event processor stopped");
    });

    log::info!("File event processor started");
}

#[cfg(test)]
#[path = "../tests/file_event_processor_tests.rs"]
mod tests;
