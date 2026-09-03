//! The two glibc allocator knobs, and the only place `libc` is named.
//!
//! They answer one question between them — how much the process keeps resident that it is no
//! longer using — and they are set against each other: pinning the trim threshold is what
//! leaves [`trim`] anything to do, since a threshold glibc has ratcheted upward leaves a
//! fully-free run sitting on the arena list where nothing hands it back.
//!
//! Compile-time no-ops off glibc-Linux, so both are safe to call unconditionally.

/// Cap the arenas and pin the mmap and trim thresholds where glibc starts them.
///
/// **Must precede the first malloc on any thread** — the logger and the runtime builder both
/// allocate, so this stays ahead of them in `main`.
///
/// The cap is 2. glibc's default `8 × num_cpus` gives every long-lived thread its own 64 MiB
/// arena, and this process runs enough of them that the committed slack is pure per-thread
/// free-list overhead. Capping trades it for malloc contention under heavy parallel
/// allocation, which an idle-most-of-the-time player doesn't have.
///
/// The other two freeze thresholds glibc otherwise ratchets upward on every mmap'd block
/// freed: one full-resolution cover decode is enough to leave every later allocation coming
/// off the arena free list, where freeing hands nothing back to the kernel. These *are*
/// glibc's initial values — pinning where the process starts, not tuning — and the trade is
/// more minor faults for less resident anonymous memory.
///
/// `M_TRIM_THRESHOLD = -1`, `M_MMAP_THRESHOLD = -3`, `M_ARENA_MAX = -8` per glibc's `malloc.h`.
pub fn pin_arenas_and_thresholds() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[allow(unsafe_code, reason = "FFI to glibc mallopt with constant args")]
    // SAFETY: no pointers cross the boundary — `int` in, `int` out. `mallopt` is
    // MT-Unsafe during init and nothing has spawned a thread yet, so no
    // concurrent allocation can observe a half-applied set.
    unsafe {
        libc::mallopt(-8, 2);
        libc::mallopt(-3, 128 * 1024);
        libc::mallopt(-1, 128 * 1024);
    }
}

/// Hand glibc's retained free-list pages back to the kernel. A well-defined no-op when there is
/// nothing to release. Cheap enough to call after any bulk free, but keep it off the UI thread
/// — it walks the arena free lists.
///
/// `tasks::heap_trim` owns the one-shot startup schedule and argues why it stays one-shot.
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
