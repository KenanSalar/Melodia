//! Tests for the unclassified-error log latch — the half of this task with no
//! `DeviceNotAvailable` to hang off.

use super::WarnedOnce;

/// The count a quiet-but-not-empty window carries.
const TRANSIENT: u64 = 3;

#[test]
fn the_first_window_warns_and_the_rest_do_not() {
    let mut warned = WarnedOnce::default();
    assert!(warned.should_warn(TRANSIENT));
    for _ in 0..5 {
        assert!(!warned.should_warn(TRANSIENT), "a repeating fault must not re-warn");
    }
}

/// An empty window drains to a defaulted report and reaches the latch as a zero,
/// which is the only thing re-arming it — without that, the first fault of a
/// session is the only one ever warned about.
#[test]
fn a_quiet_window_re_arms_the_latch() {
    let mut warned = WarnedOnce::default();
    assert!(warned.should_warn(TRANSIENT));
    assert!(!warned.should_warn(TRANSIENT));

    assert!(!warned.should_warn(0), "a quiet window is not itself a warning");
    assert!(warned.should_warn(TRANSIENT), "a second fault is warned about too");
}
