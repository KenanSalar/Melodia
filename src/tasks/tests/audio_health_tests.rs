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

/// The log line latches, so a fault that keeps repeating doesn't spend the rotation budget
/// restating itself once per window.
///
/// The lead-up is what a reporter needs out of that file, and at this rate the repeat would push
/// it out. Source-order, because nothing about the level choice is observable from [`WarnedOnce`]
/// alone — where the other half of the contract, that a quiet window reaches the latch at all,
/// is `AudioStreamHealth::drain`'s return type and needs no walk.
#[test]
fn a_repeating_fault_stops_warning_after_the_first_window() {
    let src = crate::test_support::strip_line_comments(include_str!("../audio_health.rs"));

    // Anchored on the arm and its message rather than on a whitespace-exact
    // macro call, which any reformat would retire.
    let arm = src.find("if report.other > 0 {");
    assert!(arm.is_some(), "the unclassified-error arm is gone");
    let Some(arm) = arm else { return };

    let offset =
        src.get(arm..).unwrap_or_default().find("\"audio: {} unclassified stream error(s)");
    assert!(offset.is_some(), "the unclassified-error line is gone");
    let Some(offset) = offset else { return };

    let emit = src.get(arm..arm + offset).unwrap_or_default();
    assert!(
        emit.contains("log::log!("),
        "the unclassified-error line has to be emitted at a chosen level, not a fixed one"
    );
    assert!(
        !emit.contains("log::warn!("),
        "a fixed `warn` here is the repeat that spends the rotation budget"
    );
}
