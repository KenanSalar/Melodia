use super::*;
use melodia_testkit::{block_body, strip_line_comments};

// The ordinary loop runs `NewEvents` ahead of every batch, so a pump that follows one is not
// parked, however little time has passed.
#[test]
fn a_new_events_between_two_pumps_means_the_loop_is_running() {
    assert!(!stayed_parked(Some(3), 4));
}

#[test]
fn two_pumps_with_no_new_events_between_them_are_parked() {
    assert!(stayed_parked(Some(4), 4));
}

// Nothing to compare against yet. Reading it as parked would arm a heartbeat off the first
// paint of every launch.
#[test]
fn the_first_pump_is_never_parked() {
    assert!(!stayed_parked(None, 0));
}

#[test]
fn the_heartbeat_sleeps_exactly_until_a_timer_due_within_a_frame() {
    assert_eq!(heartbeat_delay(Some(Duration::ZERO)), Some(Duration::ZERO));
    assert_eq!(heartbeat_delay(Some(Duration::from_millis(5))), Some(Duration::from_millis(5)));
    assert_eq!(heartbeat_delay(Some(PARKED_TICK_CAP)), Some(PARKED_TICK_CAP));
}

// A paint-driven pump can start a sooner timer while the thread sleeps, and nothing can wake it
// early, so a long wait is taken in frame-sized steps.
#[test]
fn the_heartbeat_never_sleeps_past_a_frame() {
    let just_past = PARKED_TICK_CAP + Duration::from_millis(1);

    assert_eq!(heartbeat_delay(Some(just_past)), Some(PARKED_TICK_CAP));
    assert_eq!(heartbeat_delay(Some(Duration::from_secs(1))), Some(PARKED_TICK_CAP));
}

#[test]
fn no_pending_timer_retires_the_heartbeat() {
    assert_eq!(heartbeat_delay(None), None);
}

/// A heartbeat posted into the ordinary loop has to find the loop moved and stop there. Pumping or
/// re-arming ahead of the check turns every drag's last heartbeat into a wake-up per timer for the
/// rest of the session. A source walk because the posted half needs a live event loop.
#[test]
fn the_heartbeat_checks_the_loop_before_it_ticks_or_rearms() {
    const FN: &str = "fn on_heartbeat()";
    let code = strip_line_comments(include_str!("../parked_loop.rs"));
    let body = code
        .find(FN)
        .and_then(|at| code[at..].find('{').map(|rel| at + rel))
        .and_then(|open| block_body(&code, open))
        .unwrap_or_default();
    let check = body.find("stayed_parked(");
    let tick = body.find("update_timers_and_animations()");
    let rearm = body.find("arm_heartbeat()");

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert!(
        check.is_some() && check < tick && check < rearm,
        "`on_heartbeat` ticks or re-arms before asking whether the loop is still parked:\n{body}"
    );
}
