//! One-shot glibc heap trim ~5 s after startup.
//!
//! Init-phase work — config load, DB pool warmup, folder-scan kickoff, Slint scene
//! build — churns short-lived allocations, and glibc's arena retains the freed pages
//! above its internal threshold rather than releasing them via
//! `madvise(MADV_DONTNEED)`. A single `malloc_trim(0)` once that has settled hands
//! the slack back to the kernel.
//!
//! **It must stay one-shot.** A periodic trim was measured against the RSS climb
//! over a long session with Now Playing open and produced no sawtooth at any
//! cadence, because that growth is not reclaimable heap: `malloc_trim` releases only
//! fully-free, page-aligned runs, and what grows is *live* — Wayland event closures
//! piling up under a high repaint rate (`wl_closure_init` via
//! `wl_display_read_events`). Reach for the producer, not the allocator.
//!
//! [`trim`] is still worth calling ad-hoc after a bulk free, such as clearing an
//! artwork cache on view exit, where the memory really is free. No-op on non-glibc
//! Linux and other platforms.

use std::time::Duration;

use crate::tasks::TaskSpawner;

/// How long to let the startup churn settle before the one-shot trim.
const STARTUP_DELAY: Duration = Duration::from_secs(5);

/// Hand glibc's retained free-list pages back to the kernel. A well-defined no-op
/// when there is nothing to release, and a compile-time one off glibc-Linux. Cheap
/// enough to call after any bulk free, but keep it off the UI thread — it walks the
/// arena free lists.
pub fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[allow(
        unsafe_code,
        reason = "FFI to glibc malloc_trim, well-defined no-op when nothing to release"
    )]
    // SAFETY: no pointers cross the boundary, and `malloc_trim` takes the arena
    // locks itself, so any thread may call it at any time.
    unsafe {
        libc::malloc_trim(0);
    }
}

pub fn spawn(spawner: &TaskSpawner) {
    spawner.spawn_cancellable(|shutdown| async move {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {}
            () = tokio::time::sleep(STARTUP_DELAY) => {
                trim();
            }
        }
    });
    log::info!("Heap trim scheduled (5s after startup)");
}
