//! The two decisions the pass makes before it can reach Deezer.
//!
//! Everything past them is a search against a host the module spells inline, so what is here is
//! the shell: a library with nothing to fetch must ask for nothing, and a second request landing
//! on a pass already running must defer to that pass rather than doubling the concurrency spent
//! on the same artists, which is what trips the quota the pacing exists to respect.

use std::sync::atomic::Ordering;

use super::{PASS_IN_FLIGHT, PASS_REQUESTED_AGAIN, fetch_artist_images};
use melodia_core::config::Paths;
use melodia_core::error::AppError;
use melodia_store::database::DbPool;
use tempfile::TempDir;

/// Both tests below read and write one process-global flag, so they are one test's worth of state
/// seen twice and must not overlap: the first clears `PASS_IN_FLIGHT` on its way out, and landing
/// that between the second's `store(true)` and its own call turns the deferral under test into a
/// real pass against a closed pool. Tokio's mutex rather than `std`'s, the guard crossing an
/// `.await` either way.
static PASS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Stages "a pass is already running" and takes it back down on the way out, whatever happens in
/// between.
///
/// A hand-written store/restore pair leaves the flag set for the rest of the binary if the
/// assertion between them fires, and every later test in this file then defers instead of running
/// — passing, and measuring nothing. It clears `PASS_REQUESTED_AGAIN` too, which the deferral sets
/// and only a live pass would otherwise read back down.
struct StagedPass;

impl StagedPass {
    fn in_flight() -> Self {
        PASS_IN_FLIGHT.store(true, Ordering::Release);
        Self
    }
}

impl Drop for StagedPass {
    fn drop(&mut self) {
        PASS_IN_FLIGHT.store(false, Ordering::Release);
        PASS_REQUESTED_AGAIN.store(false, Ordering::Release);
    }
}

/// The early return in front of the batching loop, which is also what keeps this test off the
/// network: the schema seeds an "Unknown Artist" sentinel carrying no image, so a library is only
/// free of work once every row has one.
#[tokio::test]
async fn a_library_whose_artists_all_have_images_asks_for_nothing() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let tmp = TempDir::new()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    let db = DbPool::test_pool().await?;
    sqlx::query("UPDATE artists SET image_path = '/cached/artist.jpg'")
        .execute(db.write())
        .await?;

    let fetched = fetch_artist_images(&paths, &db, &reqwest::Client::new()).await?;

    assert_eq!(fetched, 0);
    Ok(())
}

/// Driven against a closed pool, so deferring and running are distinguishable: a pass that
/// actually walked the library would fail on the first query rather than answering zero.
#[tokio::test]
async fn a_request_landing_on_a_running_pass_defers_to_it() -> Result<(), AppError> {
    let _serialized = PASS_LOCK.lock().await;
    let tmp = TempDir::new()?;
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    let db = DbPool::test_pool().await?;
    db.close().await;

    let _staged = StagedPass::in_flight();
    let deferred = fetch_artist_images(&paths, &db, &reqwest::Client::new()).await;

    assert_eq!(deferred.ok(), Some(0), "a deferred request must not walk the library itself");
    Ok(())
}
