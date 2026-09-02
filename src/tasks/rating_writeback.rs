//! Carries a star rating from the database into the file it belongs to, once the clicking stops.
//!
//! [`crate::library::ratings`] writes the row and returns; the UI repaints off that, and so do
//! smart playlists. This is the slow half, and it is slow for a structural reason: lofty
//! re-serializes the whole tag, so rating a 32 MB MP3 rewrites 32 MB. A star strip is five
//! adjacent click targets, so a user picking four stars usually sends one, two, three and four
//! on the way — hence the coalescing map and the quiet period, which turn a walk across the
//! strip into a single write of wherever the finger stopped.
//!
//! Off unless the user asked for it ([`AppState::write_ratings_to_tags`]), and the check is at
//! flush rather than at send: the switch can move while a burst is in flight, and the answer
//! that matters is the one at the moment of writing.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use crate::library::ratings;
use crate::services::describe;
use crate::state::AppState;
use crate::tasks::TaskSpawner;

/// Process-wide sender, set once by [`spawn`]. A `OnceLock` rather than a field on `AppState`
/// for the reason `play_count_flusher` uses one: the senders sit on the rating callbacks, and
/// an unset channel is exactly right for a test binary, where nothing should be touching files.
static SENDER: OnceLock<UnboundedSender<(i64, i32)>> = OnceLock::new();

/// How long the ratings have to stop moving before anything is written.
///
/// Long enough to swallow a walk across the strip, short enough that a user who rates a track and
/// immediately quits still gets the write: shutdown drains what it can pay for, which is
/// [`SHUTDOWN_FLUSH_MAX`].
const QUIET_PERIOD: Duration = Duration::from_secs(2);

/// How many tracks the shutdown drain will still write.
///
/// `shutdown::flush_tasks_and_db` gives every tracked task three seconds *together* and then
/// force-exits, and a tag write is a whole-file rewrite on a pool nothing can cancel. So a batch
/// that overruns the budget does not cost a tag, it costs a track rewritten halfway. Rating an
/// album and quitting inside the quiet period is exactly that batch, hence a cap rather than a
/// timeout: abandoning the `await` would leave the rewrite running into the exit.
///
/// Sized so the whole set finishes well inside the budget. The rows keep their stars either way,
/// and the tags the cap drops are one more click away.
const SHUTDOWN_FLUSH_MAX: usize = 8;

/// Queue `ids` for a write at `rating`. Silent no-op when the task was never spawned.
pub fn enqueue(ids: &[i64], rating: i32) {
    let Some(tx) = SENDER.get() else {
        return;
    };
    for &id in ids {
        // `send` fails only once the receiver is gone, which happens at shutdown after the
        // final drain — losing a write in that window costs the tag, never the row.
        let _ = tx.send((id, rating));
    }
}

/// Spawn the write-back loop. Idempotent; tracked by `spawner` so shutdown drains the queue
/// before the runtime goes.
pub fn spawn(spawner: &TaskSpawner, state: &AppState) {
    let (tx, rx) = mpsc::unbounded_channel();
    if SENDER.set(tx).is_err() {
        return;
    }
    let state = state.clone();
    spawner.spawn_cancellable(move |shutdown| run(rx, shutdown, state));
}

async fn run(mut rx: UnboundedReceiver<(i64, i32)>, shutdown: CancellationToken, state: AppState) {
    // Last value per track wins: the map is what makes 1→2→3→4 one write rather than four.
    let mut pending: HashMap<i64, i32> = HashMap::new();

    // Reset on every event rather than ticked on a fixed interval, so a burst straddling a tick
    // boundary still writes once. Parked until armed — a `Sleep` that has already fired stays
    // ready, and the guard is what keeps the branch from spinning on it.
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
                flush_at_exit(&state, &mut pending).await;
                return;
            }
            event = rx.recv() => {
                let Some((id, rating)) = event else {
                    flush(&state, &mut pending).await;
                    return;
                };
                pending.insert(id, rating);
                quiet.as_mut().reset(tokio::time::Instant::now() + QUIET_PERIOD);
                armed = true;
            }
            () = &mut quiet, if armed => {
                armed = false;
                flush(&state, &mut pending).await;
            }
        }
    }
}

/// The shutdown drain: write what the exit budget can cover, and say what it could not.
///
/// Which of an over-cap set survives is `HashMap` order, i.e. arbitrary. There is no better
/// answer available: the queue coalesces per track, so by the time a burst arrives here the order
/// the user clicked in is already gone, and every entry is worth the same.
async fn flush_at_exit(state: &AppState, pending: &mut HashMap<i64, i32>) {
    // The switch is read here as well as in `flush`, so a switched-off install doesn't announce
    // dropping writes it was never going to make.
    let over_budget = if state.write_ratings_to_tags.get() {
        pending.len().saturating_sub(SHUTDOWN_FLUSH_MAX)
    } else {
        0
    };
    if over_budget > 0 {
        let within_budget: HashMap<i64, i32> = pending.drain().take(SHUTDOWN_FLUSH_MAX).collect();
        *pending = within_budget;
        log::info!("rating: {over_budget} tag write(s) dropped at exit; the rows keep their stars");
    }
    flush(state, pending).await;
}

async fn flush(state: &AppState, pending: &mut HashMap<i64, i32>) {
    if pending.is_empty() {
        return;
    }
    if !state.write_ratings_to_tags.get() {
        pending.clear();
        return;
    }

    // Grouped by value because the write pass takes one edit for a whole batch — rating an
    // album is then one pass over its tracks rather than one pass each.
    let mut by_rating: HashMap<i32, Vec<i64>> = HashMap::new();
    for (id, rating) in pending.drain() {
        by_rating.entry(rating).or_default().push(id);
    }

    for (rating, ids) in by_rating {
        match ratings::write_rating_to_files(state, &ids, rating).await {
            Ok(written) => log::debug!("rating: wrote {rating} into {written} file(s)"),
            Err(e) => log::warn!("rating: write-back failed: {}", describe(&e)),
        }
    }
}
