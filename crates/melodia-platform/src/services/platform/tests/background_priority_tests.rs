//! A bulk worker's lowered priority, which has to land on that worker's thread and on no other.

use std::io;

#[cfg(target_os = "linux")]
use rustix::process::{getpid, getpriority_process};
#[cfg(target_os = "linux")]
use rustix::thread::gettid;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::ERROR_THREAD_MODE_ALREADY_BACKGROUND;

#[cfg(target_os = "linux")]
use super::BACKGROUND_NICE;
use super::lower_current_thread;
#[cfg(target_os = "windows")]
use super::lower_windows;

/// Landing on the process instead would lower the thread it started on, which is the UI's.
#[cfg(target_os = "linux")]
#[test]
fn lowering_a_thread_leaves_the_process_s_first_thread_alone() -> io::Result<()> {
    let first_before = getpriority_process(Some(getpid()))?;

    let lowered = std::thread::spawn(|| {
        lower_current_thread();
        getpriority_process(Some(gettid()))
    })
    .join()
    .map_err(|_| io::Error::other("the lowered thread panicked"))??;

    let first_after = getpriority_process(Some(getpid()))?;
    assert_eq!((lowered, first_after), (BACKGROUND_NICE, first_before));
    Ok(())
}

/// Windows refuses a second begin on a thread already in background mode. A plain thread priority
/// lets the second through, and the process's background mode refuses it under a code of its own.
#[cfg(target_os = "windows")]
#[test]
fn a_lowered_thread_is_in_background_mode() -> io::Result<()> {
    let second_begin = std::thread::spawn(|| {
        lower_current_thread();
        lower_windows()
    })
    .join()
    .map_err(|_| io::Error::other("the lowered thread panicked"))?;

    let refused = second_begin.err().and_then(|e| e.raw_os_error());
    assert_eq!(refused, Some(ERROR_THREAD_MODE_ALREADY_BACKGROUND.cast_signed()));
    Ok(())
}
