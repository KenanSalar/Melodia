//! Subscriber on `state.library_changed_tx` that reconciles the playback
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
use crate::player::actions::execute_actions;
use crate::player::event_sink::PlayerSinks;
use crate::player::rodio_backend::RodioPlayer;
use crate::player::state::{
    PlayerAction, PlayerStateHandle, lock_state, play_track_inner, stop_end_of_queue,
    with_state_emit,
};
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
    let rodio = state.rodio.clone();
    let mut rx = state.library_changed_tx.subscribe();

    spawner.spawn_cancellable(move |shutdown| async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                changed = rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let _ = rx.borrow_and_update();
                    if let Err(e) = reconcile_once(&db, &player_state, &sinks, &rodio).await {
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
    rodio: &Arc<RodioPlayer>,
) -> AppResult<()> {
    // Step 1: snapshot every track id currently referenced by the queue,
    // including the (optional) direct-play track. Brief read-only lock —
    // no with_state_emit because we're not mutating; observers should not
    // see a spurious ViewModel re-emit.
    let queued_ids: Vec<i64> = {
        let s = lock_state(player_state);
        let mut ids: Vec<i64> = Vec::with_capacity(s.queue.tracks.len() + 1);
        ids.extend(s.queue.tracks.iter().map(|t| t.id));
        if let Some(t) = &s.queue.direct_play_track {
            ids.push(t.id);
        }
        ids
    };

    if queued_ids.is_empty() {
        return Ok(());
    }

    // Step 2: ask the DB which of those ids still exist. The chunked helper
    // stays within SQLite's 999-bind cap for very long queues.
    let surviving_rows: Vec<IdRow> = crate::database::chunked_in_query(
        db.read(),
        &queued_ids,
        |placeholders| format!("SELECT id FROM tracks WHERE id IN ({placeholders})"),
    )
    .await?;
    let surviving: HashSet<i64> = surviving_rows.into_iter().map(|r| r.id).collect();

    // Step 3: compute the casualties. Snapshot ids can show duplicates (a
    // direct-play track that's also in the queue, repeated tracks…) — the
    // set is the right shape for the prune call regardless.
    let to_remove: HashSet<i64> = queued_ids
        .iter()
        .copied()
        .filter(|id| !surviving.contains(id))
        .collect();

    if to_remove.is_empty() {
        return Ok(());
    }

    log::info!("Pruning {} missing tracks from queue", to_remove.len());

    // Step 4: prune + (optionally) auto-skip in a single critical section.
    // `with_state_emit` publishes the new queue VM on `sinks.queue` and the
    // light VM on `sinks.view_model`, so the queue sheet's subscriber
    // rebuilds rows automatically.
    let actions = with_state_emit(player_state, sinks, |s| {
        let outcome = s.queue.prune_missing(&to_remove);
        if outcome.current_was_removed {
            if let Some(track) = s.queue.get_current().cloned() {
                play_track_inner(s, track, None)
            } else {
                let mut acts = stop_end_of_queue(s);
                // Clear out the player's lingering `current_track` so the
                // now-playing bar doesn't keep showing the deleted track
                // after the queue is empty. The published light ViewModel
                // mirrors `current_track`, so this propagates to the UI.
                s.current_track = None;
                acts.push(PlayerAction::PreloadGapless(None));
                acts
            }
        } else {
            Vec::new()
        }
    });

    execute_actions(actions, &**rodio, db, player_state, sinks);
    Ok(())
}
