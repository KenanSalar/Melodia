//! Two glibc heap trims after startup.
//!
//! Init-phase work — config load, DB pool warmup, folder-scan kickoff, Slint scene
//! build — churns short-lived allocations, and glibc's arena retains the freed pages
//! above its internal threshold rather than releasing them via
//! `madvise(MADV_DONTNEED)`. The first `malloc_trim(0)` hands that slack back once it
//! has settled. The second waits for the pool to close the connections the boot
//! opened, whose page caches and statements free a few MiB more after the first.
//!
//! **It must not become periodic.** A periodic trim was measured against the RSS climb
//! over a long session with Now Playing open and produced no sawtooth at any
//! cadence, because that growth is not reclaimable heap: `malloc_trim` releases only
//! fully-free, page-aligned runs, and what grows is *live* — Wayland event closures
//! piling up under a high repaint rate (`wl_closure_init` via
//! `wl_display_read_events`). Reach for the producer, not the allocator.
//!
//! [`platform::allocator::trim`](melodia_platform::services::platform::allocator::trim) is still
//! worth calling ad-hoc after a bulk free, such as clearing an artwork cache on view exit, where
//! the memory really is free. The call itself lives beside the other glibc knob; what lives here
//! is only the schedule.

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
