//! The scan bar as one run drives it.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::ScanRun;
use crate::state::{ScanControl, ScanPhase};
use melodia_store::media::ingest::scanner::ScanObserver;

fn control() -> Arc<ScanControl> {
    Arc::new(ScanControl::new(&CancellationToken::new()))
}

/// `(phase, done, total)` as the bar has it.
fn shown(control: &ScanControl) -> Option<(ScanPhase, u32, u32)> {
    control.subscribe().borrow().as_ref().map(|tick| (tick.phase, tick.done, tick.total))
}

/// Ticks inside the throttle interval are dropped, which would leave the bar short of a scan that
/// finished inside one. The count is the whole scan's, so a later chunk's last file lands on the
/// total rather than on its own chunk's share.
#[test]
fn the_scans_last_file_always_reaches_the_bar() {
    let control = control();
    let run = ScanRun::start(&control, control.token());
    run.begin_reading(2_005);
    run.chunk_read(2_000);

    run.read(5, "last.flac");

    assert_eq!(shown(&control), Some((ScanPhase::Reading, 2_005, 2_005)));
}

/// Refused under the channel's lock, so a cancel landing after the chunk loop's last check still
/// keeps the scan off the purge, which a stopped walk can't vouch for.
#[test]
fn a_cancelled_run_cannot_begin_its_last_write() {
    let control = control();
    let run = ScanRun::start(&control, control.token());
    control.cancel();

    assert!(!run.try_begin_finishing());
}

/// The run is shared with the threads that walk and read for it, and the bar goes with the last
/// of them however the scan ended, a failure included.
#[test]
fn the_bar_goes_when_the_run_is_let_go() {
    let control = control();
    let run = ScanRun::start(&control, control.token());

    drop(run);

    assert_eq!(shown(&control), None);
}
