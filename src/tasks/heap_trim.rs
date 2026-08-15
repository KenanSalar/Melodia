//! One-shot glibc heap trim ~5 s after startup.
//!
//! Init-phase work (config load, DB pool warmup, folder-scan kickoff, Slint
//! scene build) churns short-lived allocations; glibc's malloc arena retains
//! the freed pages above its internal threshold rather than releasing them
//! via `madvise(MADV_DONTNEED)`. A single `malloc_trim(0)` once the startup
//! churn has settled hands that retained slack back to the kernel.
//!
//! ## Why this is one-shot, and must stay one-shot
//!
//! It fired every 60 s from `player::handlers` once, and was cut back on a
//! measurement showing later calls returned nothing. That was re-litigated and
//! **the original measurement was right.** A periodic trim gated on the
//! visualizer being on screen was tried against the RSS climb that shows up
//! over an hour of playback with Now Playing open: the sampler showed no
//! sawtooth at the trim cadence — not one drop at a 60 s boundary in a
//! quarter-hour — and closing the view after an hour handed back less than the
//! artwork LRU's own live contents, with the rest of the hour's growth staying
//! put.
//!
//! That is the expected result, because the growth is **not** reclaimable heap.
//! `malloc_trim` can only release fully-free, page-aligned runs, and the thing
//! that actually grows is *live* allocations — Wayland event closures piling up
//! under a high repaint rate (`wl_closure_init` via `wl_display_read_events`,
//! which profiling put at the large majority of what survives a session). No
//! amount of trimming reaches memory that is still referenced. Reach for the
//! producer, not the allocator.
//!
//! [`trim`] is still worth calling ad-hoc after a bulk free (e.g. clearing an
//! artwork cache on view exit) so those pages don't linger until the next
//! process-wide event. That case is different: the memory really is free.
//!
//! No-op on non-glibc Linux and other platforms.

use std::time::Duration;

use crate::tasks::TaskSpawner;

/// How long to let the startup churn settle before the one-shot trim.
const STARTUP_DELAY: Duration = Duration::from_secs(5);

/// Hand glibc's retained free-list pages back to the kernel. A well-defined
/// no-op when there's nothing to release, and a compile-time no-op off
/// glibc-Linux. Cheap enough to call after any bulk free; keep it off the
/// UI thread regardless (it walks the arena free lists).
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
