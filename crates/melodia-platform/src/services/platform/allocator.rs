//! The allocator knobs, and the only place `libc` is named.
//!
//! Both answer how much the process keeps resident that it no longer uses, and on glibc they are
//! set against each other: pinning the trim threshold is what leaves [`trim`] anything to do, a
//! threshold glibc has ratcheted upward leaving free runs on the arena list. Compile-time no-ops
//! where a platform has no such knob, so both are safe to call unconditionally.

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

/// Hands the allocator's retained free pages back to the OS. Off the UI thread, as it walks the
/// free lists; `tasks::heap_trim` owns the schedule and argues why it is never periodic.
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

    #[cfg(target_os = "windows")]
    optimize_process_heap();
}

/// Empties the process heap's low-fragmentation caches and decommits what that frees, the nearest
/// Windows has to `malloc_trim`. `HeapCompact`, the obvious call, returns no memory to the OS.
#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "FFI to HeapSetInformation, which only releases the process heap's free memory"
)]
fn optimize_process_heap() {
    use melodia_core::error::describe;
    use windows_sys::Win32::System::Memory::{
        GetProcessHeap, HeapOptimizeResources, HeapSetInformation,
    };
    use windows_sys::Win32::System::SystemServices::{
        HEAP_OPTIMIZE_RESOURCES_CURRENT_VERSION, HEAP_OPTIMIZE_RESOURCES_INFORMATION,
    };

    let request = HEAP_OPTIMIZE_RESOURCES_INFORMATION {
        Version: HEAP_OPTIMIZE_RESOURCES_CURRENT_VERSION,
        Flags: 0,
    };
    // SAFETY: `request` is a stack local whose size matches the length passed, and the call only
    // reads it. `GetProcessHeap` answers the heap that lives as long as the process, and that heap
    // serializes its callers.
    let optimized = unsafe {
        HeapSetInformation(
            GetProcessHeap(),
            HeapOptimizeResources,
            std::ptr::from_ref(&request).cast(),
            size_of::<HEAP_OPTIMIZE_RESOURCES_INFORMATION>(),
        )
    };
    if optimized == 0 {
        log::warn!("heap trim: {}", describe(&std::io::Error::last_os_error()));
    }
}
