//! How wide a pass's pool is, which is how many threads a scan holds while it runs.

use std::num::NonZero;

use super::ScanPool;

/// `num_threads(0)` is rayon's spelling of one thread per core, so a pass with nothing to read
/// would otherwise build the widest pool there is.
#[test]
fn a_pass_over_no_files_runs_on_the_calling_thread() {
    let caller = std::thread::current().id();

    let ran_on = ScanPool::for_files(0).install(|| std::thread::current().id());

    assert_eq!(ran_on, caller);
}

/// A watcher batch of one file is the common pass, and it should cost one thread.
#[test]
fn a_pass_over_one_file_gets_one_thread() {
    assert_eq!(ScanPool::for_files(1).install(rayon::current_num_threads), 1);
}

#[test]
fn a_pass_wider_than_the_machine_gets_one_thread_per_core() {
    let cores = std::thread::available_parallelism().map_or(1, NonZero::get);

    assert_eq!(ScanPool::for_files(usize::MAX).install(rayon::current_num_threads), cores);
}
