//! What the pacer lets through, and what it refuses.
//!
//! Both halves are invisible when wrong. Sending inside a stop earns a `User-Agent` ban that lands
//! on every install at once, and refusing outside one costs a sheet nobody is told about.
//!
//! **Live clock throughout, never `start_paused`.** The state below is measured on
//! `std::time::Instant`, which tokio's paused clock does not move, so a paused test of the floor
//! does not fail: `sleep` returns at once, the verdict is recomputed against a clock that has not
//! advanced, and `acquire` spins. Every case here either takes no clock at all or returns before
//! reaching the sleep, bar the one that measures it.

use super::*;

/// A stop long enough that no scheduling delay can expire it mid-test.
const STOP: Duration = Duration::from_mins(1);

/// The step either side of a boundary. Large enough to survive `Instant`'s resolution anywhere.
const STEP: Duration = Duration::from_millis(10);

/// The floor the cases below are worked against.
///
/// The pacer's own rather than a service's: the floor arrives as a constructor argument, and each
/// service's number lives with the service that has to justify it. Reaching for one here would
/// also put a consumer's name in a primitive that knows about none of them, which
/// `crates/melodia/tests/lyrics_switch.rs` walks the tree for.
const FLOOR: Duration = Duration::from_millis(350);

/// A state whose last send was `ago` in the past, or `None` where the clock cannot reach back that
/// far, which it cannot shortly after boot.
fn sent_ago(ago: Duration) -> Option<(Inner, Instant)> {
    let now = Instant::now();
    let last = now.checked_sub(ago)?;
    Some((
        Inner {
            closed_until: None,
            last_sent: Some(last),
        },
        now,
    ))
}

#[test]
fn a_pacer_nothing_has_used_sends() {
    assert_eq!(verdict(&Inner::default(), FLOOR, Instant::now()), Verdict::Send);
}

#[test]
fn a_caller_arriving_exactly_one_floor_later_sends() {
    let Some((state, now)) = sent_ago(FLOOR) else {
        return;
    };
    assert_eq!(verdict(&state, FLOOR, now), Verdict::Send);
}

#[test]
fn a_caller_arriving_inside_the_floor_waits_out_what_is_left() {
    let Some((state, now)) = sent_ago(FLOOR.saturating_sub(STEP)) else {
        return;
    };
    assert_eq!(verdict(&state, FLOOR, now), Verdict::Wait(STEP));
}

#[test]
fn an_open_stop_refuses_with_the_time_left_on_it() {
    let now = Instant::now();
    let state = Inner {
        closed_until: Some(now + STOP),
        last_sent: None,
    };
    assert_eq!(verdict(&state, FLOOR, now), Verdict::Stopped(STOP));
}

#[test]
fn a_stop_outranks_a_floor_that_is_already_satisfied() {
    // The floor is a courtesy we impose on ourselves and the stop is the host's own answer, so a
    // caller the floor would wave through is still refused.
    let now = Instant::now();
    let Some(last) = now.checked_sub(FLOOR) else {
        return;
    };
    let state = Inner {
        closed_until: Some(now + STOP),
        last_sent: Some(last),
    };
    assert_eq!(verdict(&state, FLOOR, now), Verdict::Stopped(STOP));
}

#[test]
fn a_stop_expiring_on_this_instant_is_no_longer_a_stop() {
    // The guard is strict, so the instant a stop names is the first one that may send. Read the
    // other way it would hold every window a tick longer than the host asked for.
    let now = Instant::now();
    let state = Inner {
        closed_until: Some(now),
        last_sent: None,
    };
    assert_eq!(verdict(&state, FLOOR, now), Verdict::Send);
}

#[tokio::test]
async fn a_fresh_pacer_lets_the_first_caller_through() {
    let pacer = RequestPacer::new(FLOOR);
    assert_eq!(pacer.acquire().await, Turn::Ready);
}

#[tokio::test]
async fn an_open_stop_is_reported_rather_than_waited_out() {
    // A refusal comes back to the caller instead of parking it for the length of the stop, which
    // is what lets a lookup say so rather than hanging the panel on it.
    let pacer = RequestPacer::new(FLOOR);
    pacer.stop_for(STOP).await;

    let nearly_all_of_it = STOP.saturating_sub(STEP);
    let turn = pacer.acquire().await;
    assert!(
        matches!(turn, Turn::Stopped(left) if left > nearly_all_of_it),
        "a caller inside an open stop must be refused with the stop still on it, got {turn:?}"
    );
}

#[tokio::test]
async fn a_second_stop_extends_an_open_one_and_never_shortens_it() {
    // Two refusals in flight describe the same window, and the longer of them is the one that was
    // still true. Taking the later arrival would send us back into a window the host had closed.
    let pacer = RequestPacer::new(FLOOR);
    pacer.stop_for(STOP).await;
    pacer.stop_for(STEP).await;

    let nearly_all_of_it = STOP.saturating_sub(STEP);
    let turn = pacer.acquire().await;
    assert!(
        matches!(turn, Turn::Stopped(left) if left > nearly_all_of_it),
        "the shorter stop cut the open one down, got {turn:?}"
    );
}

#[tokio::test]
async fn a_second_caller_waits_out_the_floor_the_first_one_started() {
    // The only case that reaches the sleep, so it carries its own short floor. A lower bound
    // rather than a window: load can only make the wait longer, and a pacer that forgot to record
    // the first send would return in no time at all.
    const TIMED_FLOOR: Duration = Duration::from_millis(40);

    let pacer = RequestPacer::new(TIMED_FLOOR);
    let started = Instant::now();
    assert_eq!(pacer.acquire().await, Turn::Ready);
    assert_eq!(pacer.acquire().await, Turn::Ready);

    assert!(started.elapsed() >= TIMED_FLOOR, "the second send left no gap behind the first");
}
