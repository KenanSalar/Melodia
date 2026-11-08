use std::path::Path;
use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::time::sleep;

use super::super::IDLE_CONNECTIONS_CLOSED_WITHIN;
use super::super::open::Role;
use super::{IDLE_TIMEOUT, Lane, REAP_RETRY};
use crate::database::sql;
use melodia_core::error::AppError;

async fn call(lane: &Arc<Lane>) -> Result<(), AppError> {
    lane.run(|conn| sql::fetch_one::<i64>(conn, "SELECT 1", ())).await.map(drop)
}

async fn lane_after_a_call(dir: &Path) -> Result<Arc<Lane>, AppError> {
    let lane = Lane::file(dir.join("reaped.db"), Role::Writer, 1, Handle::current());
    call(&lane).await?;
    Ok(lane)
}

/// A reaper on a fixed period finds a connection used partway through too young, and leaves it a
/// whole period more, past [`IDLE_CONNECTIONS_CLOSED_WITHIN`].
async fn lane_called_again_midway(dir: &Path) -> Result<Arc<Lane>, AppError> {
    let lane = lane_after_a_call(dir).await?;
    sleep(IDLE_TIMEOUT / 2).await;
    call(&lane).await?;
    Ok(lane)
}

/// An idle connection holds its page cache until the reaper closes it, and `heap_trim`'s second
/// trim is timed to run after that.
#[tokio::test(start_paused = true)]
async fn a_connection_left_idle_is_closed_within_the_bound() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let lane = lane_after_a_call(tmp.path()).await?;
    assert_eq!(lane.idle().len(), 1, "the call kept no connection, so none is left to reap");

    sleep(IDLE_CONNECTIONS_CLOSED_WITHIN).await;

    assert_eq!(lane.idle().len(), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_connection_closes_a_full_timeout_after_its_last_call() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let lane = lane_called_again_midway(tmp.path()).await?;

    sleep(IDLE_TIMEOUT + REAP_RETRY / 2).await;

    assert_eq!(lane.idle().len(), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_connection_stays_open_until_a_full_timeout_after_its_last_call() -> Result<(), AppError>
{
    let tmp = tempfile::tempdir()?;
    let lane = lane_called_again_midway(tmp.path()).await?;

    sleep(IDLE_TIMEOUT.saturating_sub(REAP_RETRY / 2)).await;

    assert_eq!(lane.idle().len(), 1, "closed ahead of its timeout, so the next call reopens it");
    Ok(())
}

/// Stands in for a call between taking its permit and checking out its connection: closing that
/// connection without a permit would break the lane's one connection per permit.
#[tokio::test(start_paused = true)]
async fn an_expired_connection_with_every_permit_out_closes_within_a_retry_of_their_return()
-> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let lane = lane_after_a_call(tmp.path()).await?;
    let every_permit =
        Arc::clone(&lane.permits).try_acquire_owned().map_err(|_| AppError::DatabaseClosed)?;
    sleep(IDLE_TIMEOUT + REAP_RETRY / 2).await;
    assert_eq!(lane.idle().len(), 1, "the reaper closed a connection it held no permit for");

    drop(every_permit);
    sleep(REAP_RETRY).await;

    assert_eq!(lane.idle().len(), 0);
    Ok(())
}
