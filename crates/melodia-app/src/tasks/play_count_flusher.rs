//! Batches `play_count` and `skip_count` increments into one write per `FLUSH_INTERVAL` and one
//! at shutdown, so a run of skips doesn't queue an UPDATE apiece on the WAL writer.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::state::{AppState, Signal};
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_core::utils::now_rfc3339;
use melodia_core::utils::play_counts::{self, PlayCountEvent};
use melodia_store::database::{DbPool, queries};

/// Short enough that counts are current by the next track change, long enough to batch a burst of
/// skips.
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);

/// Asks the flusher to write what it holds now, for a write that has to see every count first, as
/// the duplicate merge does before it deletes the ids those counts name.
#[derive(Clone)]
pub struct PlayCountFlush(UnboundedSender<oneshot::Sender<()>>);

/// The flusher's end of [`PlayCountFlush`].
pub struct FlushRequests(UnboundedReceiver<oneshot::Sender<()>>);

impl PlayCountFlush {
    pub fn channel() -> (Self, FlushRequests) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), FlushRequests(rx))
    }

    /// Returns once the held counts have been written, or at once where no flusher runs.
    pub async fn flush(&self) {
        let (reply, written) = oneshot::channel();
        if self.0.send(reply).is_err() {
            return;
        }
        // A reply dropped unanswered is a flusher that stopped, which flushed on its way out.
        let _ = written.await;
    }
}

/// What the flusher listens to: the events it counts, and the asks to write them now.
struct Inbox {
    events: UnboundedReceiver<PlayCountEvent>,
    flushes: FlushRequests,
}

/// Idempotent. A play-count flush bumps `stats_changed` rather than `library_changed`, a ranking
/// moving rather than the library's structure; a skip-count flush bumps nothing, no surface
/// ranking by skips.
pub fn spawn(spawner: &TaskSpawner, state: &AppState, flushes: FlushRequests) {
    let Some(events) = play_counts::install() else {
        return;
    };
    let inbox = Inbox { events, flushes };
    let (db, stats_changed) = (state.db.clone(), state.stats_changed.clone());
    spawner.spawn_cancellable(move |shutdown| run(inbox, shutdown, db, stats_changed));
}

async fn run(mut inbox: Inbox, shutdown: CancellationToken, db: DbPool, stats_changed: Signal) {
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut play_counts: HashMap<i64, u32> = HashMap::new();
    let mut skip_counts: HashMap<i64, u32> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                while let Ok(ev) = inbox.events.try_recv() {
                    record(&mut play_counts, &mut skip_counts, ev);
                }
                flush(&db, &mut play_counts, &mut skip_counts, &stats_changed).await;
                log::info!("Play-count flusher stopped");
                return;
            }
            Some(reply) = inbox.flushes.0.recv() => {
                while let Ok(ev) = inbox.events.try_recv() {
                    record(&mut play_counts, &mut skip_counts, ev);
                }
                flush(&db, &mut play_counts, &mut skip_counts, &stats_changed).await;
                // An asker that stopped waiting has nothing to be told.
                let _ = reply.send(());
            }
            ev = inbox.events.recv() => if let Some(ev) = ev { record(&mut play_counts, &mut skip_counts, ev) } else {
                flush(&db, &mut play_counts, &mut skip_counts, &stats_changed).await;
                return;
            },
            _ = interval.tick() => {
                flush(&db, &mut play_counts, &mut skip_counts, &stats_changed).await;
            }
        }
    }
}

fn record(plays: &mut HashMap<i64, u32>, skips: &mut HashMap<i64, u32>, ev: PlayCountEvent) {
    match ev {
        PlayCountEvent::Play(id) => *plays.entry(id).or_insert(0) += 1,
        PlayCountEvent::Skip(id) => *skips.entry(id).or_insert(0) += 1,
    }
}

async fn flush(
    db: &DbPool,
    plays: &mut HashMap<i64, u32>,
    skips: &mut HashMap<i64, u32>,
    stats_changed: &Signal,
) {
    if plays.is_empty() && skips.is_empty() {
        return;
    }

    let mut play_flush_ok = false;
    if !plays.is_empty() {
        let increments: Vec<(i64, u32)> = plays.drain().collect();
        match queries::track::add_play_counts(db, &increments, &now_rfc3339()).await {
            Ok(()) => play_flush_ok = true,
            Err(e) => log::warn!("Failed to flush play counts: {}", describe(&e)),
        }
    }
    if !skips.is_empty() {
        let increments: Vec<(i64, u32)> = skips.drain().collect();
        if let Err(e) = queries::track::add_skip_counts(db, &increments).await {
            log::warn!("Failed to flush skip counts: {}", describe(&e));
        }
    }
    if play_flush_ok {
        stats_changed.bump();
    }
}

#[cfg(test)]
#[path = "tests/play_count_flusher_tests.rs"]
mod tests;
