//! Tests for the reservation's half that needs no bus: when a name request leaves the card ours,
//! which asks for it we grant, and what a hand-back waits for. Taking a card from a running session
//! manager needs one, and is tested by hand.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use zbus::fdo::RequestNameReply;

use super::{Asked, PRIORITY, REPLY_GRACE, Reservable, owns};
use crate::player::playback::output::claim::FallbackReason;

/// The object a claim serves, over a writer that closes the card when `closes` says it does and
/// counts in `releases` each time it is asked to.
fn reservable(closes: bool, asked: &Arc<Asked>, releases: &Arc<AtomicU32>) -> Reservable {
    let releases = Arc::clone(releases);
    Reservable {
        device_name: "Test Card".to_owned(),
        release: Box::new(move || {
            releases.fetch_add(1, Ordering::Relaxed);
            closes
        }),
        asked: Arc::clone(asked),
    }
}

/// A name someone else holds is an answer rather than a fault: it is what sends a claim to ask the
/// holder to let go. zbus hands the bus's `Exists` back as an error, so that arm has to read the
/// same as the reply it stands for.
#[test]
fn a_held_name_sends_the_claim_to_its_holder_and_only_a_bus_fault_refuses() {
    let rows = [
        ("taken", Ok(RequestNameReply::PrimaryOwner), Ok(true)),
        ("already ours", Ok(RequestNameReply::AlreadyOwner), Ok(true)),
        ("held, as the bus replies", Ok(RequestNameReply::Exists), Ok(false)),
        ("queued behind the holder", Ok(RequestNameReply::InQueue), Ok(false)),
        ("held, as zbus reports it", Err(zbus::Error::NameTaken), Ok(false)),
        ("the bus failing", Err(zbus::Error::Failure("gone".to_owned())), Err(FallbackReason::Io)),
    ];
    for (what, reply, expected) in rows {
        let owned = owns(reply).map_err(|e| e.reason());

        assert_eq!(owned, expected, "{what}");
    }
}

/// Only an asker above our priority gets the card, and only once the writer has closed it: a yes
/// with the card still open sends the asker into the `EBUSY` the protocol exists to avoid. The
/// session manager asks below us, which is the row that keeps the card ours.
#[test]
fn a_release_is_granted_only_above_our_priority_and_once_the_card_is_closed() {
    let rows = [
        ("the lowest priority", i32::MIN, true, false),
        ("just below ours", PRIORITY - 1, true, false),
        ("ours", PRIORITY, true, false),
        ("just above ours", PRIORITY + 1, true, true),
        ("the highest priority", i32::MAX, true, true),
        ("above ours, with the card left open", PRIORITY + 1, false, false),
    ];
    for (what, priority, closes, expected) in rows {
        let reservation = reservable(closes, &Arc::default(), &Arc::default());

        let granted = reservation.request_release(priority);

        assert_eq!(granted, expected, "{what}");
    }
}

/// The session manager asks for its card back the moment a claim takes it. Refusing that ask must
/// not stop the writer on the way, or the claim plays silence while still holding the card.
#[test]
fn an_asker_at_or_below_our_priority_never_stops_the_writer() {
    for priority in [i32::MIN, PRIORITY - 1, PRIORITY] {
        let releases = Arc::new(AtomicU32::new(0));
        let reservation = reservable(true, &Arc::default(), &releases);

        reservation.request_release(priority);

        assert_eq!(releases.load(Ordering::Relaxed), 0, "asked at {priority}");
    }
}

/// A hand-back waits for the previous holder to ask for the name, and that holder asks below our
/// priority, so the refused ask is the one the wait is for.
#[test]
fn a_refused_ask_still_counts_as_the_holder_asking_for_the_name_back() {
    let asked = Arc::new(Asked::default());
    let reservation = reservable(true, &asked, &Arc::default());
    reservation.request_release(PRIORITY - 1);

    let heard = asked.wait(Duration::ZERO);

    assert!(heard, "the hand-back missed the holder's ask");
}

#[test]
fn a_holder_that_never_asks_is_not_heard() {
    let asked = Asked::default();

    let heard = asked.wait(Duration::ZERO);

    assert!(!heard, "the hand-back heard an ask nobody made");
}

/// The answer to an ask leaves on the bus's own thread once the handler returns, so a hand-back
/// straight after it could overtake it and reach the holder before its refusal does.
#[test]
fn a_hand_back_right_after_the_ask_keeps_the_name_through_the_reply_grace() {
    let asked = Asked::default();
    let before_the_ask = Instant::now();
    asked.set();

    asked.wait(Duration::ZERO);

    let kept = before_the_ask.elapsed();
    assert!(kept >= REPLY_GRACE, "the name went back {kept:?} after the ask");
}
