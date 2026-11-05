//! The store's connections: one writer and a small band of read-only readers, each store call run
//! as one closure on the runtime's blocking pool.
//!
//! **A caller waits for its lane rather than timing out**: every closure is bounded work, and a
//! write transaction is synchronous inside its closure, so nothing awaits while one is open.

mod lane;
mod open;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, Transaction};
use tokio::runtime::Handle;

use melodia_core::error::{AppError, describe};

use super::migrate;
use lane::{IDLE_TIMEOUT, Lane};
use open::Role;

/// The fts5 compaction [`DbPool::close`] issues, named so the test proving it still runs binds the
/// same string: fts5 rejects an unknown command only at step time, and `close` can only log that.
///
/// The full collapse rather than the bounded `'merge'`: `automerge` already folds segments as
/// writes accumulate, so what reaches shutdown is the tail, and collapsing it leaves the next call a
/// no-op where a page budget only spreads the work across more shutdowns.
const FTS_OPTIMIZE: &str = "INSERT INTO tracks_fts(tracks_fts) VALUES('optimize')";

/// By when the pool has closed a connection left idle since launch: [`IDLE_TIMEOUT`] twice, the
/// reaper's first check finding it not yet idle long enough.
pub const IDLE_CONNECTIONS_CLOSED_WITHIN: Duration =
    Duration::from_secs(IDLE_TIMEOUT.as_secs() * 2);

#[derive(Clone)]
pub struct DbPool {
    read: Arc<Lane>,
    write: Arc<Lane>,
}

impl DbPool {
    /// The database at `path` behind one writer and up to `readers` read-only connections.
    pub(super) fn open(path: &Path, readers: u32) -> Result<Self, AppError> {
        let runtime = current_runtime()?;
        Ok(Self {
            read: Lane::file(path.to_path_buf(), Role::Reader, readers, runtime.clone()),
            write: Lane::file(path.to_path_buf(), Role::Writer, 1, runtime),
        })
    }

    pub(crate) async fn read<T, F>(&self, work: F) -> Result<T, AppError>
    where
        F: FnOnce(&Connection) -> Result<T, AppError> + Send + 'static,
        T: Send + 'static,
    {
        self.read.run(|conn| work(conn)).await
    }

    pub(crate) async fn write<T, F>(&self, work: F) -> Result<T, AppError>
    where
        F: FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
        T: Send + 'static,
    {
        self.write.run(work).await
    }

    /// Runs `work` in one transaction on the writer, committed when it returns `Ok` and rolled
    /// back otherwise. DEFERRED rather than the `BEGIN IMMEDIATE` a contended writer takes to fail
    /// fast: with one write connection and no second process there is no contention.
    pub(crate) async fn transaction<T, F>(&self, work: F) -> Result<T, AppError>
    where
        F: FnOnce(&Transaction<'_>) -> Result<T, AppError> + Send + 'static,
        T: Send + 'static,
    {
        self.write(|conn| {
            let tx = conn.transaction()?;
            let value = work(&tx)?;
            tx.commit()?;
            Ok(value)
        })
        .await
    }

    /// Refreshes planner statistics, compacts the search index and closes every connection, at
    /// shutdown.
    ///
    /// `PRAGMA optimize` updates `sqlite_stat1`; [`FTS_OPTIMIZE`] collapses the index's segments
    /// and drops the tombstone every track delete leaves, a smaller index for each pre-migration
    /// `VACUUM INTO` to copy. The merge is what can outgrow `flush_tasks_and_db`'s budget, and an
    /// unfinished one is rolled back on the next open, the index correct and merely un-compacted.
    /// Both **log** rather than discard, a mistyped fts5 command having no other symptom. The
    /// readers close first, so the writer is the last connection and folds the WAL into the file.
    pub async fn close(&self) {
        let optimized = self
            .write(|conn| {
                if let Err(e) = conn.execute_batch("PRAGMA optimize") {
                    log::warn!("db close: PRAGMA optimize: {}", describe(&e));
                }
                if let Err(e) = conn.execute_batch(FTS_OPTIMIZE) {
                    log::warn!("db close: fts5 optimize: {}", describe(&e));
                }
                Ok(())
            })
            .await;
        if let Err(e) = optimized {
            log::warn!("db close: {}", describe(&e));
        }
        self.read.close().await;
        self.write.close().await;
    }
}

#[doc(hidden)]
impl DbPool {
    /// An in-memory `DbPool` for tests, read and write sharing the one connection an in-memory
    /// database lives in.
    pub async fn test_pool() -> Result<Self, AppError> {
        let lane = Lane::fixed(open::open_in_memory()?, current_runtime()?);
        let pool = Self { read: Arc::clone(&lane), write: lane };
        pool.write(|conn| {
            let pending = migrate::pending(conn)?;
            migrate::apply(conn, pending)
        })
        .await?;
        Ok(pool)
    }
}

/// The runtime every call and reaper runs on, held by the pool so a call awaited off the
/// runtime's threads still runs `SQLite` on its blocking pool.
fn current_runtime() -> Result<Handle, AppError> {
    Handle::try_current().map_err(|e| AppError::io("The database pool needs a tokio runtime", e))
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
