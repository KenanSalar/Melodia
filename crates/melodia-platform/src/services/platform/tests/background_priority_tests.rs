//! A bulk worker's lowered priority, which has to land on that worker's thread and on no other.

use std::io;

use rustix::process::{getpid, getpriority_process};
use rustix::thread::gettid;

use super::{BACKGROUND_NICE, lower_current_thread};

/// Landing on the process instead would lower the thread it started on, which is the UI's.
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
