//! The shared cells `AppState` hands out.

use super::HoldCount;

/// A folder scan hands over to the reconcile it starts while both hold the scan's count, so the one
/// letting go first must not read as no scan running.
#[test]
fn a_count_stays_held_until_every_hold_on_it_drops() {
    let count = HoldCount::new();
    let first = count.hold();
    let _second = count.hold();

    drop(first);

    assert!(count.is_held());
}
