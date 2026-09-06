//! Source-order pins for `main()`'s pre-UI boot and its exit tail.
//!
//! Sibling to `boot/tests/ui_setup_tests.rs`, which pins the orderings *inside* the UI setup;
//! these three sit either side of it, in the stretch of `main()` that runs before an `AppWindow`
//! exists and the stretch that runs after the event loop returns. Neither stretch is reachable by
//! a test that executes it: everything between them wants a real audio device, a real database
//! and a Slint event loop.
//!
//! All three hold the same kind of property, and it is the kind coverage cannot see. The app
//! builds, boots, plays and exits with any of them reversed. What breaks is a memory cap that
//! silently stops applying, a forwarding launch that opens a log file it does not own, and a
//! bug report missing the tail of the session it was filed about.

const MAIN: &str = include_str!("../main.rs");

/// The offset of `needle` in `main.rs`, having first pinned that there is exactly one of it.
///
/// The count assert is what keeps the ordering assert honest: a renamed or deleted anchor would
/// otherwise make `find` answer `None` and the comparison pass or fail for a reason that has
/// nothing to do with the order. `unwrap_or` rather than `unwrap` because the crate denies it,
/// and neither default is reachable past the assert above.
fn offset_of(needle: &str) -> usize {
    assert_eq!(
        MAIN.matches(needle).count(),
        1,
        "expected exactly one `{needle}` in main.rs; the pins below read its position"
    );
    MAIN.find(needle).unwrap_or(usize::MAX)
}

/// `mallopt(M_ARENA_MAX, 2)` binds only the arenas glibc has not created yet, so it has to run
/// ahead of the first allocation on any thread. Both of the things it must precede allocate
/// immediately and on threads of their own: `flexi_logger` opens and buffers a file, and the
/// tokio builder spawns the worker pool.
///
/// Moved below either one, the cap still applies to whatever is left and the process still boots,
/// plays and exits. The only symptom is idle RSS climbing back toward the per-thread-arena
/// figure this project was started over, which no test in the tree measures and no user reports
/// as a bug.
#[test]
fn the_arena_cap_is_taken_before_anything_allocates_on_a_thread_of_its_own() {
    let cap = offset_of("pin_arenas_and_thresholds(");
    let logger = offset_of("logging::install(");
    let runtime = offset_of("tokio::runtime::Builder::new_multi_thread(");

    assert!(
        cap < logger,
        "the logger opens and buffers a file on its own thread; arenas it creates first are \
         outside the cap"
    );
    assert!(
        cap < runtime,
        "the tokio builder spawns the worker pool; arenas those threads create first are \
         outside the cap"
    );
}

/// Binding the single-instance socket is what decides whether this process is the primary or a
/// launch to be forwarded, and it has to be settled before the shared log file is opened.
///
/// A forwarding launch lives for milliseconds and then exits. Opening the rolling log on the way
/// through means two processes holding the same file, with the short-lived one able to trigger a
/// rotation the primary is still writing into, so the evidence a bug report is built from is
/// interleaved or truncated by a launch that did nothing else. Reversed, both processes still do
/// their jobs and the corruption shows up only in a log nobody reads until something else has
/// already gone wrong.
#[test]
fn the_instance_claim_is_settled_before_the_shared_log_is_opened() {
    let claim = offset_of("single_instance::claim(");
    let logger = offset_of("logging::install(");

    assert!(
        claim < logger,
        "a launch that is about to forward and exit must not open the primary's log file"
    );
}

/// Neither of the two ways out of `main()` runs a destructor: `respawn_if_requested` `exec`s on
/// Unix and never returns, and `process::exit` unwinds nothing. So the log's buffered tail is
/// only on disk if it was flushed explicitly, ahead of both.
///
/// The lost part is the end of the session, which is the part a crash or a hang is described by
/// and the part `--logs` exists to hand the user. Everything up to the last rotation still
/// survives, so the file looks normal and reads as though the session simply stopped.
#[test]
fn the_log_is_flushed_before_either_way_out_of_main() {
    let flush = offset_of("logging::flush(");
    let respawn = offset_of("respawn_if_requested(");
    let exit = offset_of("std::process::exit(0);");

    assert!(flush < respawn, "`exec` replaces the image and never returns to flush anything");
    assert!(flush < exit, "`process::exit` runs no destructor that would flush the buffer");
}
