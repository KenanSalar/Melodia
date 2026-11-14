//! The pass's progress as the Settings card paints it, and the Stop the user holds over it.

use super::{LoudnessControl, MeasurePhase, MeasureProgress};

fn progress(phase: MeasurePhase, done: u32) -> MeasureProgress {
    MeasureProgress { phase, done, total: 5 }
}

fn shown(control: &LoudnessControl) -> MeasureProgress {
    *control.subscribe_progress().borrow()
}

#[test]
fn stop_turns_a_measuring_row_to_stopped_and_leaves_an_idle_one() {
    let rows = [
        (MeasurePhase::Running, MeasurePhase::Stopped),
        (MeasurePhase::WaitingForScan, MeasurePhase::Stopped),
        (MeasurePhase::Idle, MeasurePhase::Idle),
    ];
    for (before, after) in rows {
        let control = LoudnessControl::new();
        control.publish(progress(before, 2));

        control.stop();

        assert_eq!(shown(&control).phase, after, "{before:?}");
    }
}

/// A measurement committing just after Stop publishes a running count, which must not paint over
/// the Stopped row the user just asked for.
#[test]
fn a_running_count_published_after_stop_leaves_it_stopped() {
    let control = LoudnessControl::new();
    control.publish(progress(MeasurePhase::Running, 2));
    control.stop();

    control.publish(progress(MeasurePhase::Running, 3));

    assert_eq!(shown(&control), progress(MeasurePhase::Stopped, 2));
}

#[test]
fn a_resumed_pass_shows_its_count_again() {
    let control = LoudnessControl::new();
    control.publish(progress(MeasurePhase::Running, 2));
    control.stop();
    control.resume();

    control.publish(progress(MeasurePhase::Running, 3));

    assert_eq!(shown(&control), progress(MeasurePhase::Running, 3));
}

/// Nothing else restarts a stopped pass this session, new tracks arriving included.
#[test]
fn resume_wakes_the_pass() {
    let control = LoudnessControl::new();
    let kick = control.subscribe_kick();

    control.resume();

    assert!(matches!(kick.has_changed(), Ok(true)));
}
