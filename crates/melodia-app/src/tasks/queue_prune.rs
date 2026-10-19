//! Subscriber on `state.library_changed` that reconciles the playback
//! queue against the live `tracks` table. When the file watcher (or any
//! other library mutation) removes a row, this task drops the matching
//! id out of the queue. If the currently-playing track
//! was among the casualties, playback auto-skips to the next surviving
//! entry — or stops if the queue is now empty.
//!
//! Pruning publishes through the existing `sinks.queue` watch channel via
//! `with_state_emit`, so the queue sheet's `ViewModel` subscriber redraws
//! automatically. No new UI subscriber is needed.

use std::collections::HashSet;
use std::sync::Arc;

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::{AppResult, describe};
use melodia_engine::player::engine::actions::emit_and_execute;
use melodia_engine::player::engine::backend::PlaybackEngine;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::{
    PlayerAction, PlayerStateHandle, lock_state, stop_end_of_queue,
};
use melodia_engine::player::engine::types::PlaybackSource;
use melodia_store::database::{DbPool, queries};

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let db = state.db.clone();
    let player_state = state.player_state.clone();
    let sinks = state.sinks.clone();
    let engine = state.engine.clone();
    let mut rx = state.library_changed.subscribe();

    spawner.spawn_cancellable(move |shutdown| async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                changed = rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    if let Err(e) = reconcile_once(&db, &player_state, &sinks, &engine).await {
                        log::warn!("queue_prune reconcile failed: {}", describe(&e));
                    }
                }
            }
        }
        log::info!("Queue prune task stopped");
    });

    log::info!("Queue prune task started");
}

/// One reconciliation pass: figure out which queued tracks no longer have a
/// DB row, prune them, and auto-skip if the currently-playing entry was
/// among them. Holds the player-state lock only inside the two
/// `with_state_emit` calls (one for the snapshot, one for the mutation) so
/// the DB roundtrip in the middle doesn't pin readers/writers.
async fn reconcile_once(
    db: &DbPool,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    engine: &Arc<PlaybackEngine>,
) -> AppResult<()> {
    // Step 1: snapshot every track id currently referenced by the queue.
    // Brief read-only lock — no with_state_emit because we're not mutating;
    // observers should not see a spurious ViewModel re-emit.
    let queued_ids = lock_state(player_state).queue.play_ids();

    if queued_ids.is_empty() {
        return Ok(());
    }

    // Step 2: ask the DB which of those ids still exist.
    let surviving = queries::track::existing_ids(db, &queued_ids).await?;

    // Step 3: compute the casualties. The snapshot can hold the same id
    // more than once (a track queued twice) — the set is the right shape
    // for the prune call regardless.
    let to_remove: HashSet<i64> =
        queued_ids.iter().copied().filter(|id| !surviving.contains(id)).collect();

    if to_remove.is_empty() {
        return Ok(());
    }

    log::info!("Pruning {} missing tracks from queue", to_remove.len());

    // Step 4: where the playing entry goes, the record of the one playback moves on to, so the
    // start below needn't wait on the window task.
    let successor = match unheld_successor(player_state, &to_remove) {
        Some(id) => queries::track::get_track_summaries_by_ids(db, &[id]).await?,
        None => Vec::new(),
    };

    // Step 5: prune + (optionally) auto-skip in a single critical section.
    // `with_state_emit` publishes the new queue VM on `sinks.queue` and the
    // light VM on `sinks.view_model`, so the queue sheet's subscriber
    // rebuilds rows automatically.
    emit_and_execute(&**engine, player_state, sinks, |s| {
        let outcome = s.queue.prune_missing(&to_remove);
        s.window.seat(&s.queue.window_ids(), successor.into_iter().map(Arc::new));
        // A station leaves the queue seated underneath rather than playing from it, so a row
        // going missing out of it is not a playback event at all: reacting would stop the
        // station and take it off screen over a track it was never playing.
        if !outcome.current_was_removed || !s.source_allows(PlaybackSource::advances_queue) {
            return Vec::new();
        }
        if let Some(id) = s.queue.current_id() {
            return s.start_queued(id, false, 0);
        }
        let mut acts = stop_end_of_queue(s);
        // Clear the deck so the now-playing bar stops showing a track the queue no longer
        // holds. The published light ViewModel projects this field, so it propagates to the UI.
        s.source = None;
        acts.push(PlayerAction::PreloadGapless(None));
        acts
    });
    Ok(())
}

/// The track the queue lands on once `gone` is pruned, where the playing entry is among `gone` and
/// the window holds no record for what follows it. Asked of a copy, so the answer is
/// `prune_missing`'s own; a playing entry going missing is rare enough to pay for one.
fn unheld_successor(player_state: &PlayerStateHandle, gone: &HashSet<i64>) -> Option<i64> {
    let s = lock_state(player_state);
    if !s.queue.current_id().is_some_and(|id| gone.contains(&id)) {
        return None;
    }
    let mut after = s.queue.clone();
    after.prune_missing(gone);
    after.current_id().filter(|&id| s.record(id).is_none())
}

#[cfg(test)]
#[path = "tests/queue_prune_tests.rs"]
mod tests;
