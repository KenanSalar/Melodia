//! One-shot glibc heap trim ~5 s after startup.
//!
//! Init-phase work (config load, DB pool warmup, folder-scan kickoff, Slint
//! scene build) churns short-lived allocations; glibc's malloc arena retains
//! the freed pages above its internal threshold rather than releasing them
//! via `madvise(MADV_DONTNEED)`. A single `malloc_trim(0)` once the startup
//! churn has settled hands that retained slack back to the kernel.
//!
//! Previously this fired every 60 s from `player::handlers` — measurement
//! showed only the first call returned meaningful pages, every subsequent
//! one was a no-op. One-shot is sufficient. No-op on non-glibc Linux and
//! other platforms.
//!
//! [`trim`] is also called ad-hoc after a bulk free elsewhere (e.g. clearing
//! an artwork cache on view exit) so the released pages don't linger in the
//! arena until the next process-wide event.

use std::time::Duration;

use crate::tasks::TaskSpawner;

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
    unsafe {
        libc::malloc_trim(0);
    }
}

pub fn spawn(spawner: &TaskSpawner) {
    spawner.spawn_cancellable(|shutdown| async move {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {}
            () = tokio::time::sleep(Duration::from_secs(5)) => {
                trim();
            }
        }
    });
    log::info!("Heap trim scheduled (5s after startup)");
}
