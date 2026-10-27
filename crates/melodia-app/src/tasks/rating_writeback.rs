//! Carries a star rating from the database into its file once the clicking stops.
//!
//! [`crate::library::ratings`] writes the row; this is the slow half, since lofty re-serializes the
//! whole tag and so rewrites the whole file. Picking four stars on a strip of five usually sends
//! one through four on the way, and the coalescing map and the quiet period turn that walk into
//! one write. The switch ([`AppState::write_ratings_to_tags`]) is read at flush rather than at
//! send, the answer that matters being the one at the moment of writing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use crate::library::ratings;
use crate::state::{AppState, SharedFlag};
use crate::tasks::TaskSpawner;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::event_bridge::EventBridge;
use melodia_core::utils::self_writes::SelfWrites;
use melodia_store::database::DbPool;

/// Claimed once by [`spawn`]. A bridge rather than an `AppState` field, as `play_count_flusher`'s
/// is: an unclaimed channel is right for a test binary, where nothing should touch files.
static BRIDGE: EventBridge<(i64, i32)> = EventBridge::new();

/// Owned clones rather than an `&AppState`, so the coalescing, the quiet period and the flush-time
/// switch read can be driven by a test.
#[derive(Clone)]
struct Writeback {
    db: DbPool,
    artwork_dir: PathBuf,
    cover_cache: CoverCache,
    self_writes: Arc<SelfWrites>,
    write_to_tags: SharedFlag,
}

impl Writeback {
    fn from_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
            artwork_dir: state.paths.artwork_dir.clone(),
            cover_cache: state.cover_cache.clone(),
            self_writes: state.self_writes.clone(),
            write_to_tags: state.write_ratings_to_tags.clone(),
        }
    }

    async fn write(&self, ids: &[i64], rating: i32) -> Result<usize, AppError> {
        ratings::write_rating_to_files(
            &self.db,
            &self.artwork_dir,
            &self.cover_cache,
            &self.self_writes,
            ids,
            rating,
        )
        .await
    }
}

/// Long enough to swallow a walk across the strip, short enough that rating and then quitting
/// still writes, up to [`SHUTDOWN_FLUSH_MAX`].
const QUIET_PERIOD: Duration = Duration::from_secs(2);

/// How many tracks the shutdown drain still writes. `shutdown::flush_tasks_and_db` gives every
/// task three seconds together, and a tag write is a whole-file rewrite nothing can cancel, so an
/// overrun costs a track rewritten halfway: hence a cap rather than a timeout. The rows keep their
/// stars either way.
const SHUTDOWN_FLUSH_MAX: usize = 8;

/// A silent no-op when the task was never spawned.
pub fn enqueue(ids: &[i64], rating: i32) {
    for &id in ids {
        // A send fails only after shutdown's final drain, costing the tag, never the row.
        BRIDGE.send((id, rating));
    }
}

/// Idempotent; tracked, so shutdown drains the queue first.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let Some(rx) = BRIDGE.install() else {
        return;
    };
    let writeback = Writeback::from_state(state);
    spawner.spawn_cancellable(move |shutdown| run(rx, shutdown, writeback));
}

async fn run(
    mut rx: UnboundedReceiver<(i64, i32)>,
    shutdown: CancellationToken,
    writeback: Writeback,
) {
    // Last value per track wins: the map is what makes 1→2→3→4 one write rather than four.
    let mut pending: HashMap<i64, i32> = HashMap::new();

    // Reset per event rather than ticked, so a burst straddling a tick still writes once. `armed`
    // guards it, a fired `Sleep` staying ready.
    let quiet = tokio::time::sleep(QUIET_PERIOD);
    tokio::pin!(quiet);
    let mut armed = false;

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                while let Ok((id, rating)) = rx.try_recv() {
                    pending.insert(id, rating);
                }
                flush_at_exit(&writeback, &mut pending).await;
                return;
            }
            event = rx.recv() => {
                let Some((id, rating)) = event else {
                    flush(&writeback, &mut pending).await;
                    return;
                };
                pending.insert(id, rating);
                quiet.as_mut().reset(tokio::time::Instant::now() + QUIET_PERIOD);
                armed = true;
            }
            () = &mut quiet, if armed => {
                armed = false;
                flush(&writeback, &mut pending).await;
            }
        }
    }
}

/// Writes what the exit budget covers. Which of an over-cap set survives is arbitrary: coalescing
/// has already lost the click order, and every entry is worth the same.
async fn flush_at_exit(writeback: &Writeback, pending: &mut HashMap<i64, i32>) {
    // Read here too, so a switched-off install doesn't report dropping writes it never makes.
    let over_budget = if writeback.write_to_tags.get() {
        pending.len().saturating_sub(SHUTDOWN_FLUSH_MAX)
    } else {
        0
    };
    if over_budget > 0 {
        let within_budget: HashMap<i64, i32> = pending.drain().take(SHUTDOWN_FLUSH_MAX).collect();
        *pending = within_budget;
        log::info!("rating: {over_budget} tag write(s) dropped at exit; the rows keep their stars");
    }
    flush(writeback, pending).await;
}

async fn flush(writeback: &Writeback, pending: &mut HashMap<i64, i32>) {
    if pending.is_empty() {
        return;
    }
    if !writeback.write_to_tags.get() {
        pending.clear();
        return;
    }

    // Grouped by value, one write pass taking a whole batch.
    let mut by_rating: HashMap<i32, Vec<i64>> = HashMap::new();
    for (id, rating) in pending.drain() {
        by_rating.entry(rating).or_default().push(id);
    }

    for (rating, ids) in by_rating {
        match writeback.write(&ids, rating).await {
            Ok(written) => log::debug!("rating: wrote {rating} into {written} file(s)"),
            Err(e) => log::warn!("rating: write-back failed: {}", describe(&e)),
        }
    }
}

#[cfg(test)]
#[path = "tests/rating_writeback_tests.rs"]
mod tests;
