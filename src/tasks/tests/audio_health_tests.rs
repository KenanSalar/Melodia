//! Tests for the storm escalation — the half of the device-lost notice that has
//! to work without a `DeviceNotAvailable` to hang off.

use super::{BACKEND_ERROR_STORM, STORM_WINDOWS_TO_REPORT, Storm, StormWatch};

/// The count a quiet-but-not-empty window carries. Deliberately far below the
/// threshold: a stream still producing sound reports one error per failed write.
const TRANSIENT: u64 = 3;

#[test]
fn a_single_storm_window_says_nothing() {
    let mut watch = StormWatch::default();
    assert_eq!(watch.classify(BACKEND_ERROR_STORM), Storm::Below);
}

#[test]
fn a_sustained_storm_reports_once() {
    let mut watch = StormWatch::default();
    for _ in 1..STORM_WINDOWS_TO_REPORT {
        assert_eq!(watch.classify(BACKEND_ERROR_STORM), Storm::Below);
    }
    assert_eq!(
        watch.classify(BACKEND_ERROR_STORM),
        Storm::Lost,
        "the last window is the one that reports"
    );

    // A device that stays gone keeps storming, and the user has been told.
    for _ in 0..5 {
        assert_eq!(
            watch.classify(BACKEND_ERROR_STORM),
            Storm::Ongoing,
            "a latched storm must not re-toast"
        );
    }
}

/// The failure this guards is a burst that clears on its own raising a sticky
/// toast the user then has to dismiss.
#[test]
fn a_storm_that_clears_never_reports() {
    let mut watch = StormWatch::default();
    for _ in 1..STORM_WINDOWS_TO_REPORT {
        assert_eq!(watch.classify(BACKEND_ERROR_STORM), Storm::Below);
    }
    assert_eq!(watch.classify(TRANSIENT), Storm::Below);
    assert_eq!(
        watch.classify(BACKEND_ERROR_STORM),
        Storm::Below,
        "the count starts over after a window under the bar"
    );
}

/// An empty window drains to `None` and reaches the watch as a zero, which is
/// the only thing re-arming the latch — without it the first disconnect of a
/// session is the only one ever reported.
#[test]
fn a_quiet_window_re_arms_the_latch() {
    let mut watch = StormWatch::default();
    for _ in 1..STORM_WINDOWS_TO_REPORT {
        let _ = watch.classify(BACKEND_ERROR_STORM);
    }
    assert_eq!(watch.classify(BACKEND_ERROR_STORM), Storm::Lost);

    assert_eq!(watch.classify(0), Storm::Below);
    for _ in 1..STORM_WINDOWS_TO_REPORT {
        assert_eq!(watch.classify(BACKEND_ERROR_STORM), Storm::Below);
    }
    assert_eq!(
        watch.classify(BACKEND_ERROR_STORM),
        Storm::Lost,
        "a second disconnect is reported too"
    );
}

/// The threshold is a floor, not a target — an ALSA spin arrives orders of
/// magnitude above it, and one count below it must stay a log line.
#[test]
fn the_threshold_is_a_floor_and_the_spin_clears_it() {
    let mut watch = StormWatch::default();
    assert_eq!(
        watch.classify(BACKEND_ERROR_STORM - 1),
        Storm::Below,
        "below the floor is not a storm"
    );

    let mut spinning = StormWatch::default();
    for _ in 1..STORM_WINDOWS_TO_REPORT {
        assert_eq!(spinning.classify(u64::MAX), Storm::Below);
    }
    assert_eq!(spinning.classify(u64::MAX), Storm::Lost, "the count a wedged worker loop reaches");
}

/// The empty window has to reach [`StormWatch`], and only source order says so.
///
/// [`a_quiet_window_re_arms_the_latch`] proves the watch handles a zero; nothing
/// in it can see the *call site* stop handing one over. Restoring the obvious
/// `let Some(report) = health.drain() else { continue };` above the classify
/// compiles, passes every other test in this file, and makes the first
/// disconnect of a session the only one ever reported.
///
/// Comments are stripped first, the `nav_persist_tests` reason: the prose around
/// that call names both statements, so a raw search would be satisfied by the
/// warning rather than by the code.
#[test]
fn an_empty_window_is_classified_before_the_drain_is_unwrapped() {
    let src = crate::test_support::strip_line_comments(include_str!("../audio_health.rs"));

    assert!(
        !src.contains("health.drain() else"),
        "the drain has to be bound before it is unwrapped, so an empty window still classifies"
    );

    let classify = src.find("storms.classify(");
    let unwrap = src.find("let Some(report) = report else");
    assert!(classify.is_some(), "`spawn` no longer classifies each window through `StormWatch`");
    assert!(unwrap.is_some(), "`spawn` no longer binds the drain before unwrapping it");
    let (Some(classify), Some(unwrap)) = (classify, unwrap) else {
        return;
    };

    assert!(
        classify < unwrap,
        "the classify has to run above the `else {{ continue }}`, or an empty window is skipped"
    );

    let call = src.get(classify..unwrap).unwrap_or_default();
    assert!(
        call.contains("map_or(0"),
        "an absent report has to reach the watch as a zero — that is what re-arms the latch"
    );
}

/// The log line latches with the toast, so a device that stays gone doesn't
/// spend the rotation budget restating itself once per window.
///
/// The lead-up is what a reporter needs out of that file, and at this rate the
/// repeat would push it out. Source-order again: nothing about the level choice
/// is observable from [`StormWatch`] alone.
#[test]
fn a_storm_stops_warning_once_the_user_has_been_told() {
    let src = crate::test_support::strip_line_comments(include_str!("../audio_health.rs"));

    assert!(
        src.contains("storm == Storm::Ongoing"),
        "the backend-error line has to drop below `warn` once the notice has gone out"
    );
    assert!(
        src.contains("storm == Storm::Lost"),
        "the notice fires on the crossing window alone, never on `Ongoing`"
    );
    // Anchored on the arm and its message rather than on a whitespace-exact
    // macro call, which any reformat would retire.
    let arm = src.find("if report.other > 0 {");
    assert!(arm.is_some(), "the backend-error arm is gone");
    let Some(arm) = arm else { return };

    let offset = src.get(arm..).unwrap_or_default().find("\"audio: {} backend stream error(s)");
    assert!(offset.is_some(), "the backend-error line is gone");
    let Some(offset) = offset else { return };

    let emit = src.get(arm..arm + offset).unwrap_or_default();
    assert!(
        emit.contains("log::log!("),
        "the backend-error line has to be emitted at a chosen level, not a fixed one"
    );
    assert!(
        !emit.contains("log::warn!("),
        "a fixed `warn` here is the repeat that spends the rotation budget"
    );
}
