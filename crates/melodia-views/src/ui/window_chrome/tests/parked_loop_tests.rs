use super::*;
use melodia_testkit::{block_after, strip_line_comments};

// Each loop-tick count below is a `NewEvents` count: equal counts mean no `NewEvents` ran between
// the two calls, the one signal a Win32 modal loop gives off.

// Nothing to compare against yet. Reading it as parked would arm a heartbeat off the first paint
// of every launch.
#[test]
fn the_first_pump_never_arms() {
    let mut watch = ParkedWatch::new();

    let arms = watch.pumped(0);

    assert!(!arms);
}

// The ordinary loop runs `NewEvents` ahead of every batch, so a pump that follows one is not
// parked, however little time has passed.
#[test]
fn a_pump_after_new_events_arms_nothing() {
    let mut watch = ParkedWatch::new();
    watch.pumped(3);

    let arms = watch.pumped(4);

    assert!(!arms);
}

#[test]
fn a_second_pump_with_no_new_events_between_arms_the_heartbeat() {
    let mut watch = ParkedWatch::new();
    watch.pumped(4);

    let arms = watch.pumped(4);

    assert!(arms);
}

/// Every paint of a drag is a parked pump, so without this refusal each one queued another
/// heartbeat behind the pending one and the drag woke once per paint for every timer.
#[test]
fn a_parked_pump_arms_nothing_while_a_heartbeat_is_pending() {
    let mut watch = ParkedWatch::new();
    watch.pumped(4);
    watch.armed(4);

    let arms = watch.pumped(4);

    assert!(!arms);
}

/// The case the heartbeat exists for: the pointer has stopped, no event is coming, and only the
/// heartbeat's own re-arm keeps the swap timer and the crossfade moving.
#[test]
fn a_heartbeat_landing_while_still_parked_ticks() {
    let mut watch = ParkedWatch::new();
    watch.armed(4);

    let ticks = watch.heartbeat(4);

    assert!(ticks);
}

/// The drag ended while the heartbeat slept. Ticking here would re-arm it in the ordinary loop, and
/// keep doing so for as long as any Slint timer runs.
#[test]
fn a_heartbeat_landing_after_new_events_retires() {
    let mut watch = ParkedWatch::new();
    watch.armed(4);

    let ticks = watch.heartbeat(5);

    assert!(!ticks);
}

#[test]
fn a_heartbeat_with_nothing_armed_does_nothing() {
    let mut watch = ParkedWatch::new();

    let ticks = watch.heartbeat(0);

    assert!(!ticks);
}

// One arm, one tick: a second delivery against the same arm is not a second heartbeat.
#[test]
fn a_heartbeat_spends_the_arm_it_lands_on() {
    let mut watch = ParkedWatch::new();
    watch.armed(4);
    watch.heartbeat(4);

    let ticks = watch.heartbeat(4);

    assert!(!ticks);
}

/// A retired heartbeat has to leave the watch able to arm again, or every drag after the first
/// stalls exactly as the unfixed pump did.
#[test]
fn a_retired_heartbeat_leaves_the_next_drag_free_to_arm() {
    let mut watch = ParkedWatch::new();
    watch.pumped(4);
    watch.armed(4);
    watch.heartbeat(5);
    watch.pumped(7);

    let arms = watch.pumped(7);

    assert!(arms);
}

#[test]
fn no_pending_timer_retires_the_heartbeat() {
    assert_eq!(heartbeat_delay(None), None);
}

// Either side of the cap and on it, the real constant rather than a round number.
#[test]
fn a_timer_due_within_a_frame_is_slept_toward_exactly() {
    let just_under = PARKED_TICK_CAP.saturating_sub(Duration::from_millis(1));

    assert_eq!(heartbeat_delay(Some(Duration::ZERO)), Some(Duration::ZERO));
    assert_eq!(heartbeat_delay(Some(just_under)), Some(just_under));
    assert_eq!(heartbeat_delay(Some(PARKED_TICK_CAP)), Some(PARKED_TICK_CAP));
}

// A paint-driven pump can start a sooner timer while the thread sleeps, and nothing can wake it
// early, so a long wait is taken in frame-sized steps.
#[test]
fn a_timer_due_past_a_frame_is_slept_toward_a_frame_at_a_time() {
    let just_past = PARKED_TICK_CAP + Duration::from_millis(1);

    assert_eq!(heartbeat_delay(Some(just_past)), Some(PARKED_TICK_CAP));
    assert_eq!(heartbeat_delay(Some(Duration::from_secs(1))), Some(PARKED_TICK_CAP));
}

/// The watch holds the decision, but only a heartbeat that asks it before ticking is gated by it.
/// Ticking or re-arming ahead of the question turns a drag's last heartbeat into a wake-up per timer
/// for the rest of the session. A source walk because the posted half needs a live event loop.
#[test]
fn the_heartbeat_asks_the_watch_before_it_ticks_or_rearms() {
    const FN: &str = "fn on_heartbeat()";
    let code = strip_line_comments(include_str!("../parked_loop.rs"));
    let body = block_after(&code, FN);
    let ask = body.find(".heartbeat(");
    let tick = body.find("update_timers_and_animations()");
    let rearm = body.find("arm_heartbeat(");

    assert!(!body.is_empty(), "no `{FN}` found: the walk is broken, not the code");
    assert!(
        ask.is_some() && ask < tick && ask < rearm,
        "`on_heartbeat` ticks or re-arms before asking the watch whether the loop is still \
         parked:\n{body}"
    );
}
