//! Tests for the task's two pure halves: the unclassified-error log latch, and the watch that
//! reads a stalled callback as a lost device.

use super::{STALL_POLLS, StallWatch, WarnedOnce};

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
    let src = melodia_testkit::strip_line_comments(include_str!("../audio_health.rs"));

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

/// The block count a stream has reached when the watch first reads it.
const BEATING: u64 = 7;

/// A second of silence from the callback is the only sign a `PipeWire` restart under the ALSA
/// plugin leaves, so the watch has to fire on exactly the poll that completes the stall.
#[test]
fn a_stall_fires_on_the_poll_that_completes_it() {
    let mut stall = StallWatch::default();
    assert!(!stall.observe(BEATING), "the first read is movement, not a stall");
    for poll in 1..STALL_POLLS {
        assert!(
            !stall.observe(BEATING),
            "fired after {poll} quiet poll(s), short of the threshold"
        );
    }
    assert!(stall.observe(BEATING), "a full stall went unseen");
}

/// Past the stall the output may have been left with no stream at all, and a watch that kept
/// firing would retry a reopen every poll for the rest of the session.
#[test]
fn a_stall_fires_once_however_long_it_lasts() {
    let mut stall = StallWatch::default();
    for _ in 0..=STALL_POLLS {
        stall.observe(BEATING);
    }
    for _ in 0..STALL_POLLS * 3 {
        assert!(!stall.observe(BEATING), "the same stall fired twice");
    }
}

/// Any beat re-arms it, so the stream a recovery opened is watched from its first block.
#[test]
fn movement_re_arms_the_watch() {
    let mut stall = StallWatch::default();
    for _ in 0..=STALL_POLLS {
        stall.observe(BEATING);
    }
    assert!(!stall.observe(BEATING + 1), "movement is not a stall");
    for _ in 1..STALL_POLLS {
        stall.observe(BEATING + 1);
    }
    assert!(stall.observe(BEATING + 1), "a second stall after a recovery went unseen");
}
