//! Lowered scheduling priority for a thread doing bulk work, so it takes only what playback and the
//! UI leave over. Set by the thread on itself, so it lands on that thread alone.

use melodia_core::error::describe;

/// Lowers the calling thread's CPU priority, and its I/O priority where the OS ties the two. A
/// failure leaves the thread at normal priority, which costs smoothness rather than correctness,
/// so it is logged instead of returned.
///
/// macOS is left alone: its per-thread call, `pthread_set_qos_class_self_np`, is reachable only
/// through `libc` or a hand-written `extern` block.
pub fn lower_current_thread() {
    let lowered: std::io::Result<()> = cfg_select! {
        target_os = "linux" => lower_linux(),
        target_os = "windows" => lower_windows(),
        _ => Ok(()),
    };
    if let Err(e) = lowered {
        log::warn!("Background thread priority left as it was: {}", describe(&e));
    }
}

/// The weakest nice value, which still takes every idle cycle. A thread with no I/O priority of its
/// own gets best-effort at a level its nice value sets, which BFQ honours and mq-deadline and
/// `none` don't.
#[cfg(target_os = "linux")]
const BACKGROUND_NICE: i32 = 19;

/// On Linux `setpriority` given a thread id sets that thread alone, whatever POSIX says of the
/// process.
#[cfg(target_os = "linux")]
fn lower_linux() -> std::io::Result<()> {
    rustix::process::setpriority_process(Some(rustix::thread::gettid()), BACKGROUND_NICE)?;
    Ok(())
}

/// Background mode lowers the thread's CPU and I/O priority together.
#[cfg(target_os = "windows")]
#[allow(unsafe_code, reason = "FFI to SetThreadPriority on the calling thread's own pseudo-handle")]
fn lower_windows() -> std::io::Result<()> {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };

    // SAFETY: no pointer crosses. `GetCurrentThread` answers a pseudo-handle that names the calling
    // thread and needs no closing, and it is used on that same thread.
    let lowered = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) };
    if lowered == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
#[path = "tests/background_priority_tests.rs"]
mod tests;
