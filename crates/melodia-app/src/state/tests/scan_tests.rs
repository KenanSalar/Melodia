//! What a cancel stops, and what the scan bar shows across one.

use tokio_util::sync::CancellationToken;

use super::{ScanControl, ScanPhase, ScanProgressTick};

fn tick(phase: ScanPhase) -> ScanProgressTick {
    ScanProgressTick { phase, found: 0, done: 0, total: 0, current_file: String::new() }
}

fn shown_phase(control: &ScanControl) -> Option<ScanPhase> {
    control.subscribe().borrow().as_ref().map(|tick| tick.phase)
}

#[test]
fn a_cancel_stops_a_scan_already_running() {
    let control = ScanControl::new(&CancellationToken::new());
    let running = control.token();

    control.cancel();

    assert!(running.is_cancelled());
}

/// The epoch is what lets one button stop a boot reconcile and a rescan alike without leaving
/// every scan after it born cancelled.
#[test]
fn a_scan_started_after_a_cancel_starts_clean() {
    let control = ScanControl::new(&CancellationToken::new());
    control.cancel();

    assert!(!control.token().is_cancelled());
}

/// The epoch a cancel installs descends from the shutdown token rather than from the one it
/// replaced, or quitting would no longer reach a scan started after the user stopped one.
#[test]
fn quitting_stops_a_scan_started_after_a_cancel() {
    let shutdown = CancellationToken::new();
    let control = ScanControl::new(&shutdown);
    control.cancel();
    let scan = control.token();

    shutdown.cancel();

    assert!(scan.is_cancelled());
}

/// A cancelled import is withdrawn on this count, and a quit cutting one short must leave it for
/// the next launch to finish.
#[test]
fn only_the_users_cancel_moves_the_count() {
    let shutdown = CancellationToken::new();
    let control = ScanControl::new(&shutdown);

    shutdown.cancel();
    let after_quit = control.user_cancels();
    control.cancel();

    assert_eq!((after_quit, control.user_cancels()), (0, 1));
}

/// A worker reading its last file as the cancel lands would otherwise paint "Reading" over the
/// "Stopping" the cancel put up.
#[test]
fn a_cancelled_scan_cannot_paint_over_the_stopping_bar() {
    let control = ScanControl::new(&CancellationToken::new());
    let scan = control.token();
    control.publish(&scan, tick(ScanPhase::Reading));
    control.cancel();

    let published = control.publish(&scan, tick(ScanPhase::Reading));

    assert_eq!((published, shown_phase(&control)), (false, Some(ScanPhase::Stopping)));
}

/// The last write brings the library's counts in line and can't stop part way, so the bar keeps
/// saying it is finishing rather than promise a stop that isn't coming.
#[test]
fn a_cancel_shows_stopping_only_over_a_phase_that_can_stop() {
    let cases = [
        (ScanPhase::Discovering, ScanPhase::Stopping),
        (ScanPhase::Reading, ScanPhase::Stopping),
        (ScanPhase::Finishing, ScanPhase::Finishing),
        (ScanPhase::Stopping, ScanPhase::Stopping),
    ];

    for (before, after) in cases {
        let control = ScanControl::new(&CancellationToken::new());
        control.publish(&control.token(), tick(before));
        control.cancel();
        assert_eq!(shown_phase(&control), Some(after), "cancelled while {before:?}");
    }
}

#[test]
fn a_cancel_with_no_scan_running_puts_no_bar_up() {
    let control = ScanControl::new(&CancellationToken::new());

    control.cancel();

    assert_eq!(shown_phase(&control), None);
}
