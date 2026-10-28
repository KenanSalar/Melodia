//! The two glibc allocator knobs, and the only place `libc` is named.
//!
//! Both answer how much the process keeps resident that it no longer uses, and they are set
//! against each other: pinning the trim threshold is what leaves [`trim`] anything to do, a
//! threshold glibc has ratcheted upward leaving free runs on the arena list. Compile-time no-ops
//! off glibc Linux, so both are safe to call unconditionally.

/// Caps the arenas and pins the mmap and trim thresholds where glibc starts them.
///
/// **Must precede the first malloc on any thread**, the logger and the runtime builder both
/// allocating.
///
/// The cap is 2: glibc's `8 × num_cpus` gives every long-lived thread its own 64 MiB arena, slack
/// traded here for malloc contention an idle-most-of-the-time player doesn't have. The thresholds
/// would otherwise ratchet up on every mmap'd block freed, one full-resolution cover decode leaving
/// every later allocation on a free list that hands nothing back. These *are* glibc's initial
/// values, pinned rather than tuned.
///
/// `M_TRIM_THRESHOLD = -1`, `M_MMAP_THRESHOLD = -3`, `M_ARENA_MAX = -8` per glibc's `malloc.h`.
pub fn pin_arenas_and_thresholds() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[allow(unsafe_code, reason = "FFI to glibc mallopt with constant args")]
    // SAFETY: no pointers cross the boundary, `int` in and `int` out. `mallopt` is MT-Unsafe during
    // init and no thread exists yet, so nothing can observe a half-applied set.
    unsafe {
        libc::mallopt(-8, 2);
        libc::mallopt(-3, 128 * 1024);
        libc::mallopt(-1, 128 * 1024);
    }
}

/// Hands glibc's retained free-list pages back to the kernel. Off the UI thread, as it walks the
/// arena free lists; `tasks::heap_trim` owns the schedule and argues why it is never periodic.
pub fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[allow(
        unsafe_code,
        reason = "FFI to glibc malloc_trim, well-defined no-op when nothing to release"
    )]
    // SAFETY: no pointers cross the boundary, and `malloc_trim` takes the arena locks itself, so
    // any thread may call it at any time.
    unsafe {
        libc::malloc_trim(0);
    }
}
