//! The shared cells `AppState` hands out.

use super::{HoldCount, Shadowed};

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

/// Two picks in flight run on the pool in either order, and whichever takes the lock last has to
/// apply and persist the newest. That holds only while the lock spans the apply, the engine and
/// the settings file each taking a lock of their own.
#[test]
fn apply_latest_holds_the_apply_lock_until_the_apply_returns() {
    let shadow = Shadowed::new(0);

    let held = shadow.apply_latest(|picked| *picked, |_| shadow.applying.try_lock().is_none());

    assert!(held, "a second task could take the apply lock while the first was applying");
}

/// A click takes the pick's own lock, and the apply behind it can be a device open.
#[test]
fn a_pick_can_land_while_an_apply_is_in_flight() {
    let shadow = Shadowed::new(0);

    let free = shadow.apply_latest(|picked| *picked, |_| shadow.picked.try_lock().is_some());

    assert!(free, "a click would wait out the device open in flight");
}
