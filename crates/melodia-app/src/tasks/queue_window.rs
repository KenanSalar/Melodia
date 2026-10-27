//! Keeps the player's window holding the records around the queue's current slot, so a skip, a
//! track end or a gapless stage starts from one in hand, and runs the starts that waited for one.
//!
//! A queue publish fetches what the window lacks, a library tick re-reads all of it (a retag, a
//! star or a moved file changes the record), and an owed start (`PlayerAction::StartWhenResolved`)
//! fetches its track. Every pass ends by asking for that start, so a failed fetch is retried by the
//! next wake.

use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_core::error::{AppResult, describe};
use melodia_engine::player::engine::actions::start_awaited;
use melodia_engine::player::engine::event_sink::PlayerSinks;
use melodia_engine::player::engine::state::{
    PlayerStateHandle, WindowFill, seat_window_records, window_wanted,
};
use melodia_store::database::{DbPool, queries};

pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let db = state.db.clone();
    let player_state = state.player_state.clone();
    let sinks = state.sinks.clone();
    let engine = state.engine.clone();
    let mut queue_rx = state.sinks.queue.subscribe();
    let mut awaited_rx = state.sinks.awaited_start.subscribe();
    let mut library_rx = state.library_changed.subscribe();

    spawner.spawn_cancellable(move |shutdown| async move {
        loop {
            let fill = tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                changed = awaited_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    WindowFill::Missing
                }
                changed = queue_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    WindowFill::Missing
                }
                changed = library_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    WindowFill::All
                }
            };
            if let Err(e) = fill_window(&db, &player_state, &sinks, fill).await {
                log::warn!("Queue window fetch failed: {}", describe(&e));
            }
            // Copied out before the start, which can publish on this same channel.
            let awaited = *awaited_rx.borrow();
            if let Some(awaited) = awaited {
                start_awaited(&*engine, &player_state, &sinks, awaited);
            }
        }
        log::info!("Queue window task stopped");
    });

    log::info!("Queue window task started");
}

/// Fetch the records `fill` asks for and seat those the queue still wants once they land.
async fn fill_window(
    db: &DbPool,
    player_state: &PlayerStateHandle,
    sinks: &PlayerSinks,
    fill: WindowFill,
) -> AppResult<()> {
    let wanted = window_wanted(player_state, fill);
    if wanted.is_empty() {
        return Ok(());
    }
    let fresh = queries::track::get_track_summaries_by_ids(db, &wanted).await?;
    seat_window_records(player_state, sinks, fresh);
    Ok(())
}

#[cfg(test)]
#[path = "tests/queue_window_tests.rs"]
mod tests;
