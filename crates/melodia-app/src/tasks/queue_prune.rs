//! Drops the ids of deleted tracks from the queue on every `library_changed`, moving on, or
//! stopping, when the playing track was one. The prune publishes through `emit_and_execute`, so
//! the queue panel redraws off its existing subscriber.

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

/// The player-state lock is held for the snapshot and for the mutation, never across the database
/// round trip between them.
async fn reconcile_once(
    db: &DbPool,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    engine: &PlaybackEngine,
) -> AppResult<()> {
    // A bare lock rather than an emit: nothing changes, so observers shouldn't see a re-emit.
    let queued_ids = lock_state(player_state).queue.play_ids();

    if queued_ids.is_empty() {
        return Ok(());
    }

    // A set either way: the snapshot can hold a track queued twice.
    let to_remove = queries::track::missing_ids(db, &queued_ids).await?;

    if to_remove.is_empty() {
        return Ok(());
    }

    log::info!("Pruning {} missing tracks from queue", to_remove.len());

    // The record playback moves on to, so the start below needn't wait on the window task.
    let successor = match unheld_successor(player_state, &to_remove) {
        Some(id) => queries::track::get_track_summaries_by_ids(db, &[id]).await?,
        None => Vec::new(),
    };

    emit_and_execute(engine, player_state, sinks, |s| {
        let outcome = s.queue.prune_missing(&to_remove);
        s.window.seat(&s.queue.window_ids(), successor.into_iter().map(Arc::new));
        // A station plays over the queue rather than from it, so a row missing from it is no
        // playback event: reacting would stop the station over a track it was never playing.
        if !outcome.current_was_removed || !s.source_allows(PlaybackSource::advances_queue) {
            return Vec::new();
        }
        if let Some(id) = s.queue.current_id() {
            return s.start_queued(id, false, 0);
        }
        let mut acts = stop_end_of_queue(s);
        // So the bar stops showing a track the queue no longer holds.
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
