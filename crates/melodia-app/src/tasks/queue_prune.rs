//! Subscriber on `state.library_changed` that reconciles the playback
//! queue against the live `tracks` table. When the file watcher (or any
//! other library mutation) removes a row, this task drops the matching
//! `Arc<TrackSummary>` out of the queue. If the currently-playing track
//! was among the casualties, playback auto-skips to the next surviving
//! entry — or stops if the queue is now empty.
//!
//! Pruning publishes through the existing `sinks.queue` watch channel via
//! `with_state_emit`, so the queue sheet's `ViewModel` subscriber redraws
//! automatically. No new UI subscriber is needed.

use std::collections::HashSet;
use std::sync::Arc;

use sqlx::FromRow;

use crate::database::DbPool;
use crate::error::AppResult;
use crate::player::engine::actions::emit_and_execute;
use crate::player::engine::backend::PlaybackEngine;
use crate::player::engine::event_sink::PlayerSinks;
use crate::player::engine::state::{
    PlayerAction, PlayerStateHandle, lock_state, play_track_inner, stop_end_of_queue,
};
use crate::player::engine::types::PlaybackSource;
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// `chunked_in_query` row shape for `SELECT id FROM tracks WHERE id IN (…)`.
#[derive(FromRow)]
struct IdRow {
    id: i64,
}

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
                        log::warn!("queue_prune reconcile failed: {e}");
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
    let queued_ids: Vec<i64> = {
        let s = lock_state(player_state);
        s.queue.tracks.iter().map(|t| t.id).collect()
    };

    if queued_ids.is_empty() {
        return Ok(());
    }

    // Step 2: ask the DB which of those ids still exist. The chunked helper
    // stays within SQLite's 999-bind cap for very long queues.
    let surviving_rows: Vec<IdRow> =
        crate::database::chunked_in_query(db.read(), &queued_ids, |placeholders| {
            format!("SELECT id FROM tracks WHERE id IN ({placeholders})")
        })
        .await?;
    let surviving: HashSet<i64> = surviving_rows.into_iter().map(|r| r.id).collect();

    // Step 3: compute the casualties. The snapshot can hold the same id
    // more than once (a track queued twice) — the set is the right shape
    // for the prune call regardless.
    let to_remove: HashSet<i64> =
        queued_ids.iter().copied().filter(|id| !surviving.contains(id)).collect();

    if to_remove.is_empty() {
        return Ok(());
    }

    log::info!("Pruning {} missing tracks from queue", to_remove.len());

    // Step 4: prune + (optionally) auto-skip in a single critical section.
    // `with_state_emit` publishes the new queue VM on `sinks.queue` and the
    // light VM on `sinks.view_model`, so the queue sheet's subscriber
    // rebuilds rows automatically.
    emit_and_execute(&**engine, player_state, sinks, |s| {
        let outcome = s.queue.prune_missing(&to_remove);
        // A station leaves the queue seated underneath rather than playing from it, so a row
        // going missing out of it is not a playback event at all: reacting would stop the
        // station and take it off screen over a track it was never playing.
        if !outcome.current_was_removed || !s.source_allows(PlaybackSource::advances_queue) {
            return Vec::new();
        }
        if let Some(track) = s.queue.get_current().cloned() {
            return play_track_inner(s, track, None);
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
