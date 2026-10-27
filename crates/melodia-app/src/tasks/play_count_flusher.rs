//! Batches `play_count` and `skip_count` increments into one write per `FLUSH_INTERVAL` and one
//! at shutdown, so a run of skips doesn't queue an UPDATE apiece on the WAL writer.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use crate::state::Signal;
use crate::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_core::utils::now_rfc3339;
use melodia_core::utils::play_counts::{self, PlayCountEvent};
use melodia_store::database::{DbPool, queries};

/// Short enough that counts are current by the next track change, long enough to batch a burst of
/// skips.
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);

/// Idempotent. A play-count flush bumps `stats_changed` rather than `library_changed`, a ranking
/// moving rather than the library's structure; a skip-count flush bumps nothing, no surface
/// ranking by skips.
pub fn spawn(spawner: &TaskSpawner, db: DbPool, stats_changed: Signal) {
    let Some(rx) = play_counts::install() else {
        return;
    };
    spawner.spawn_cancellable(move |shutdown| run(rx, shutdown, db, stats_changed));
}

async fn run(
    mut rx: UnboundedReceiver<PlayCountEvent>,
    shutdown: CancellationToken,
    db: DbPool,
    stats_changed: Signal,
) {
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut play_counts: HashMap<i64, u32> = HashMap::new();
    let mut skip_counts: HashMap<i64, u32> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                while let Ok(ev) = rx.try_recv() {
                    record(&mut play_counts, &mut skip_counts, ev);
                }
                flush(&db, &mut play_counts, &mut skip_counts, &stats_changed).await;
                log::info!("Play-count flusher stopped");
                return;
            }
            ev = rx.recv() => if let Some(ev) = ev { record(&mut play_counts, &mut skip_counts, ev) } else {
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
