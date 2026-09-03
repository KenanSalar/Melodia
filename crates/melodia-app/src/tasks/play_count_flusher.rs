//! Batched flusher for `play_count` / `skip_count` UPDATEs.
//!
//! Without this, every track end / skip fires a separate write-pool round-trip
//! (see `PlayerAction::UpdatePlayCount` / `UpdateSkipCount` in
//! `src/player/actions.rs`). A user rapidly skipping through a playlist can
//! easily fire 10 UPDATEs in a few seconds, each waiting for the WAL writer.
//!
//! The flusher absorbs both kinds of events on an `UnboundedSender` and
//! coalesces them into a single multi-row `CASE WHEN` UPDATE every 2 s (or on
//! shutdown). Senders never block — the channel is unbounded — and the queue
//! shape is `Vec<(track_id, kind)>` so duplicate events on the same track
//! just stack into the same `play_count + N` increment.

use std::collections::HashMap;
use std::time::Duration;

use sqlx::AssertSqlSafe;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use crate::state::Signal;
use crate::tasks::TaskSpawner;
use melodia_core::utils::now_rfc3339;
use melodia_core::utils::play_counts::{self, PlayCountEvent};
use melodia_store::database::DbPool;

/// How often to flush pending events. Short enough that play counts feel
/// up-to-date in the UI on the next track change, long enough to batch a
/// realistic burst of skips into a single write.
const FLUSH_INTERVAL: Duration = Duration::from_secs(2);

/// Spawn the flusher. Idempotent — a second call is a no-op (re-using the
/// existing sender). Tracked by `spawner` so shutdown awaits the final
/// flush before the runtime is dropped.
///
/// `stats_changed` is bumped after every successful play-count flush
/// so subscribers (Favorites hero mosaic, future "Most Played" widgets,
/// …) re-fetch when the ranking changes. Deliberately NOT
/// `library_changed`: a play-count write changes a ranking, not the
/// library's structure, and bumping the structural channel forced every
/// view refresher + `queue_prune` to run after every played song.
/// Skip-count flushes do not bump — no UI surface depends on skip counts.
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
                // Drain any remaining events without blocking.
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
        let now = now_rfc3339();
        match flush_play_counts(db, plays, &now).await {
            Ok(()) => play_flush_ok = true,
            Err(e) => log::warn!("Failed to flush play counts: {e}"),
        }
        plays.clear();
    }
    if !skips.is_empty() {
        if let Err(e) = flush_skip_counts(db, skips).await {
            log::warn!("Failed to flush skip counts: {e}");
        }
        skips.clear();
    }
    // Bump only on a successful play-count flush — the Favorites hero
    // mosaic (top-4 most-played) and any future "Most Played" widget
    // pulls from `play_count`, so a fresh write means the ranking may
    // have shifted. Skips are intentionally out of scope; no UI surface
    // ranks by skip_count today, and bumping for nothing would cost a
    // wasted re-fetch on every skip burst.
    if play_flush_ok {
        stats_changed.bump();
    }
}

/// Build a single `UPDATE … SET play_count = play_count + CASE id … END,
/// last_played = ? WHERE id IN (…)` and execute it.
async fn flush_play_counts(
    db: &DbPool,
    counts: &HashMap<i64, u32>,
    now: &str,
) -> Result<(), melodia_core::error::AppError> {
    // Stay below SQLite's 999 bind cap: 2 binds per row (id for CASE, id for
    // IN) + 1 for `now`.
    const MAX_ROWS: usize = (melodia_store::database::SQLITE_BIND_LIMIT - 1) / 2;
    let entries: Vec<(i64, u32)> = counts.iter().map(|(&k, &v)| (k, v)).collect();
    for chunk in entries.chunks(MAX_ROWS) {
        let mut sql = String::from("UPDATE tracks SET play_count = play_count + CASE id");
        for _ in chunk {
            sql.push_str(" WHEN ? THEN ?");
        }
        sql.push_str(" END, last_played = ? WHERE id IN (");
        for i in 0..chunk.len() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push('?');
        }
        sql.push(')');

        let mut q = sqlx::query(AssertSqlSafe(sql));
        for &(id, n) in chunk {
            q = q.bind(id).bind(i64::from(n));
        }
        q = q.bind(now);
        for &(id, _) in chunk {
            q = q.bind(id);
        }
        q.persistent(false).execute(db.write()).await?;
    }
    Ok(())
}

async fn flush_skip_counts(
    db: &DbPool,
    counts: &HashMap<i64, u32>,
) -> Result<(), melodia_core::error::AppError> {
    const MAX_ROWS: usize = melodia_store::database::SQLITE_BIND_LIMIT / 2;
    let entries: Vec<(i64, u32)> = counts.iter().map(|(&k, &v)| (k, v)).collect();
    for chunk in entries.chunks(MAX_ROWS) {
        let mut sql = String::from("UPDATE tracks SET skip_count = skip_count + CASE id");
        for _ in chunk {
            sql.push_str(" WHEN ? THEN ?");
        }
        sql.push_str(" END WHERE id IN (");
        for i in 0..chunk.len() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push('?');
        }
        sql.push(')');

        let mut q = sqlx::query(AssertSqlSafe(sql));
        for &(id, n) in chunk {
            q = q.bind(id).bind(i64::from(n));
        }
        for &(id, _) in chunk {
            q = q.bind(id);
        }
        q.persistent(false).execute(db.write()).await?;
    }
    Ok(())
}
