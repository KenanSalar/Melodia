use std::time::Duration;

use tokio::runtime::Handle;

use super::super::IDLE_CONNECTIONS_CLOSED_WITHIN;
use super::super::open::Role;
use super::Lane;
use crate::database::sql;
use melodia_core::error::AppError;

/// An idle connection holds its page cache until the reaper closes it, and `heap_trim`'s second
/// trim is timed to run after that.
#[tokio::test(start_paused = true)]
async fn a_connection_left_idle_is_closed_within_the_bound() -> Result<(), AppError> {
    let tmp = tempfile::tempdir()?;
    let lane = Lane::file(tmp.path().join("reaped.db"), Role::Writer, 1, Handle::current());
    lane.run(|conn| sql::fetch_one::<i64>(conn, "SELECT 1", ())).await?;
    assert_eq!(lane.idle().len(), 1, "the call kept no connection, so none is left to reap");

    tokio::time::sleep(IDLE_CONNECTIONS_CLOSED_WITHIN + Duration::from_secs(1)).await;

    assert_eq!(lane.idle().len(), 0);
    Ok(())
}
