//! One lane of connections: a bound on how many calls run at once, the idle connections kept for
//! the next, and every call run on the runtime's blocking pool.
//!
//! **A call's permit travels with its connection** into the blocking task and is released only
//! after the connection is back, so a dropped future never frees a permit while its connection
//! still works, and a lane never holds more connections than permits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::Connection;
use tokio::runtime::Handle;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use melodia_core::error::{AppError, describe};

use super::open::{self, Role};

/// How long a connection sits idle before the reaper closes it, taking its map of the file and its
/// page cache with it. `scripts/measure-footprint.py` sizes both its settles past it.
pub(super) const IDLE_TIMEOUT: Duration = Duration::from_mins(1);

/// The shortest the reaper sleeps, so an expired connection it can't close yet, every permit being
/// out with a call, is retried rather than spun on.
pub(super) const REAP_RETRY: Duration = Duration::from_secs(1);

pub(super) struct Lane {
    capacity: u32,
    permits: Arc<Semaphore>,
    /// Most recently used last, so a burst's surplus is what sits idle long enough to reap.
    idle: Mutex<Vec<Idle>>,
    source: Source,
    runtime: Handle,
    closing: AtomicBool,
    /// Wakes the reaper ahead of its deadline: when the idle stack gains its first connection, and
    /// when the lane closes or drops.
    reaper_wake: Arc<Notify>,
}

struct Idle {
    conn: Connection,
    since: Instant,
}

/// Where a lane gets a connection its idle stack can't hand out.
enum Source {
    File {
        path: PathBuf,
        role: Role,
    },
    /// The one connection the lane was built with, an in-memory database that reopening would
    /// replace with an empty one.
    Fixed,
}

impl Lane {
    /// Up to `capacity` connections to `path`, opened on first use and closed once idle for
    /// [`IDLE_TIMEOUT`], so neither a scan's cached pages nor a boot's prefetch burst outlive the
    /// work. A cold reopen costs nothing at this scale.
    pub(super) fn file(path: PathBuf, role: Role, capacity: u32, runtime: Handle) -> Arc<Self> {
        let lane = Arc::new(Self::new(Source::File { path, role }, capacity, Vec::new(), runtime));
        spawn_reaper(&lane);
        lane
    }

    /// One lane over `conn` alone, never reaped.
    pub(super) fn fixed(conn: Connection, runtime: Handle) -> Arc<Self> {
        let idle = vec![Idle { conn, since: Instant::now() }];
        Arc::new(Self::new(Source::Fixed, 1, idle, runtime))
    }

    fn new(source: Source, capacity: u32, idle: Vec<Idle>, runtime: Handle) -> Self {
        Self {
            capacity,
            permits: Arc::new(Semaphore::new(capacity as usize)),
            idle: Mutex::new(idle),
            source,
            runtime,
            closing: AtomicBool::new(false),
            reaper_wake: Arc::new(Notify::new()),
        }
    }

    /// Runs `work` on one of the lane's connections, on the blocking pool. Fails with
    /// [`AppError::DatabaseClosed`] once the lane is closing, and resumes a panic in `work`.
    pub(super) async fn run<T, F>(self: &Arc<Self>, work: F) -> Result<T, AppError>
    where
        F: FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
        T: Send + 'static,
    {
        if self.closing.load(Ordering::Acquire) {
            return Err(AppError::DatabaseClosed);
        }
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| AppError::DatabaseClosed)?;
        let lane = Arc::clone(self);
        let task = self.runtime.spawn_blocking(move || lane.run_holding(permit, work));
        match task.await {
            Ok(result) => result,
            Err(failure) if failure.is_panic() => std::panic::resume_unwind(failure.into_panic()),
            Err(_) => Err(AppError::DatabaseClosed),
        }
    }

    fn run_holding<T>(
        &self,
        permit: OwnedSemaphorePermit,
        work: impl FnOnce(&mut Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut conn = self.checkout()?;
        let result = work(&mut conn);
        self.check_in(conn);
        // Only now, so the next caller finds the connection rather than opening another.
        drop(permit);
        result
    }

    /// Returns `conn` to the idle stack, waking the reaper if the stack was empty, the one time it
    /// sleeps with no deadline.
    fn check_in(&self, conn: Connection) {
        let mut idle = self.idle();
        let was_empty = idle.is_empty();
        idle.push(Idle { conn, since: Instant::now() });
        drop(idle);
        if was_empty {
            self.reaper_wake.notify_one();
        }
    }

    fn checkout(&self) -> Result<Connection, AppError> {
        if let Some(idle) = self.idle().pop() {
            return Ok(idle.conn);
        }
        match &self.source {
            Source::File { path, role } => Ok(open::open(path, *role)?),
            Source::Fixed => Err(AppError::DatabaseClosed),
        }
    }

    /// Fails every later call at once, waits out the calls already running, then closes each
    /// connection.
    pub(super) async fn close(&self) {
        self.closing.store(true, Ordering::Release);
        self.reaper_wake.notify_one();
        if let Ok(running) = self.permits.acquire_many(self.capacity).await {
            running.forget();
        }
        self.permits.close();
        let idle = std::mem::take(&mut *self.idle());
        close_all(&self.runtime, idle.into_iter().map(|entry| entry.conn).collect()).await;
    }

    async fn reap(&self) {
        let (expired, permits) = self.take_expired();
        close_all(&self.runtime, expired).await;
        drop(permits);
    }

    /// The connections idle for [`IDLE_TIMEOUT`], each with a permit held until it has closed so
    /// the lane never holds more connections than permits.
    fn take_expired(&self) -> (Vec<Connection>, Vec<OwnedSemaphorePermit>) {
        let mut idle = self.idle();
        let mut permits = Vec::new();
        for entry in idle.iter() {
            if entry.since.elapsed() < IDLE_TIMEOUT {
                break;
            }
            let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else {
                break;
            };
            permits.push(permit);
        }
        let expired = idle.drain(..permits.len()).map(|entry| entry.conn).collect();
        (expired, permits)
    }

    /// When the longest-idle connection reaches [`IDLE_TIMEOUT`], or `None` while none is idle.
    fn next_expiry(&self) -> Option<Instant> {
        self.idle().first().map(|entry| entry.since + IDLE_TIMEOUT)
    }

    fn idle(&self) -> MutexGuard<'_, Vec<Idle>> {
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Lane {
    // Lets a reaper waiting with no deadline find the lane gone rather than wait for a check-in.
    fn drop(&mut self) {
        self.reaper_wake.notify_one();
    }
}

/// Closes each connection once idle for [`IDLE_TIMEOUT`]: sleeps until the longest-idle one gets
/// there or, with none idle, until one is, and ends when the lane closes or drops.
fn spawn_reaper(lane: &Arc<Lane>) {
    let lane_ref = Arc::downgrade(lane);
    let wake = Arc::clone(&lane.reaper_wake);
    lane.runtime.spawn(async move {
        loop {
            // Strong only for the pass, so a pool dropped while the reaper sleeps drops its lane.
            let expiry = {
                let Some(lane) = lane_ref.upgrade() else {
                    return;
                };
                if lane.closing.load(Ordering::Acquire) {
                    return;
                }
                lane.reap().await;
                lane.next_expiry()
            };
            match expiry {
                Some(deadline) => {
                    let deadline = deadline.max(Instant::now() + REAP_RETRY);
                    tokio::select! {
                        () = tokio::time::sleep_until(deadline) => {}
                        () = wake.notified() => {}
                    }
                }
                None => wake.notified().await,
            }
        }
    });
}

/// Closes `conns` on the blocking pool, since the last connection to a database checkpoints its
/// WAL on the way out.
async fn close_all(runtime: &Handle, conns: Vec<Connection>) {
    if conns.is_empty() {
        return;
    }
    let closing = runtime.spawn_blocking(move || {
        for conn in conns {
            if let Err((_, e)) = conn.close() {
                log::warn!("db close: {}", describe(&e));
            }
        }
    });
    if let Err(e) = closing.await {
        log::warn!("db close: {}", describe(&e));
    }
}

#[cfg(test)]
#[path = "tests/lane_tests.rs"]
mod tests;
