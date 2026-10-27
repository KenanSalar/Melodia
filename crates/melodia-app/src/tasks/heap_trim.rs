//! Two glibc heap trims after startup: one once the boot's short-lived allocations have settled,
//! which glibc's arena keeps rather than returns, and one after the pool closes the connections
//! the boot opened.
//!
//! **It must not become periodic.** The RSS climb over a long session with Now Playing open is
//! live memory, Wayland event closures piling up under a high repaint rate, which `malloc_trim`
//! cannot release: reach for the producer. An ad-hoc
//! [`trim`](melodia_platform::services::platform::allocator::trim) after a bulk free is still fine.

use std::time::Duration;

use tokio::time::Instant;

use crate::tasks::TaskSpawner;
use melodia_platform::services::platform::allocator::trim;
use melodia_store::database::IDLE_CONNECTIONS_CLOSED_WITHIN;

/// How long to let the startup churn settle before the first trim.
const AFTER_STARTUP: Duration = Duration::from_secs(10);

/// How long past the pool closing the boot's connections the second trim waits, the reaper's tick
/// being counted from the pool's creation rather than from this task's.
const AFTER_POOL_CLOSES: Duration = Duration::from_secs(15);

const TRIMS: [Duration; 2] =
    [AFTER_STARTUP, IDLE_CONNECTIONS_CLOSED_WITHIN.saturating_add(AFTER_POOL_CLOSES)];

pub fn spawn(spawner: &TaskSpawner) {
    let started = Instant::now();
    spawner.spawn_cancellable(move |shutdown| async move {
        for after in TRIMS {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep_until(started + after) => trim(),
            }
        }
    });
    log::info!(
        "Heap trims scheduled ({}s and {}s after startup)",
        TRIMS[0].as_secs(),
        TRIMS[1].as_secs()
    );
}
