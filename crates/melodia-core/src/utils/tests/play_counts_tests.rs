//! Tests for the process-wide play-count bridge.
//!
//! `BRIDGE` is a `OnceLock`, so this is one test rather than several: the claim is one-shot per
//! process, and a second test could only assert that it had already happened.

use super::*;

/// The engine sends on every track end and every skip, and for the window before `boot::tasks`
/// installs the flusher there is nothing on the other side. That has to be a dropped event and a
/// `false`, never a panic, because the caller is `execute_actions` running under the player lock.
#[test]
fn the_bridge_drops_events_until_the_flusher_claims_it() {
    assert!(
        !try_send(PlayCountEvent::Play(1)),
        "an unclaimed bridge reports no delivery, and the count is simply lost"
    );

    // Defensive `else { return }` rather than a failure: with the receiver already claimed there
    // is no delivery to verify. Nothing else in this binary claims it, `execute_actions` being a
    // melodia-engine test.
    let Some(mut rx) = install() else { return };
    assert!(install().is_none(), "a second claim must not replace the flusher's receiver");

    assert!(try_send(PlayCountEvent::Play(7)));
    assert!(try_send(PlayCountEvent::Skip(7)));

    // Unbounded sends are synchronous, so both are queued by the time we look.
    let delivered: Vec<PlayCountEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(delivered.len(), 2, "both events must arrive, got {delivered:?}");
    assert!(
        matches!(delivered[0], PlayCountEvent::Play(7))
            && matches!(delivered[1], PlayCountEvent::Skip(7)),
        "a play and a skip on one track are different counts and must stay in order: {delivered:?}"
    );
}
