//! Tests for the process-wide toast bridge sender.
//!
//! `SENDER` is a process-global `OnceLock`, so this runs as a single test to
//! avoid cross-test coupling on the one-shot initializer. Once `init` sets the
//! global sender, though, *any* other test in the lib binary that calls
//! `notify` (e.g. the `execute_actions` decode-failure tests) routes its request
//! into this test's receiver too. So the delivery check filters to this test's
//! own two sentinel details and tolerates interleaved foreign toasts rather than
//! asserting on exact `try_recv` position — otherwise a concurrent `notify`
//! landing between our sends would flake the assertion.

use super::*;

/// This test's own toast details. Distinct enough not to collide with any
/// real producer's `notify` (temp file paths, error messages), so we can pick
/// exactly our two requests out of the shared global channel.
const SENTINEL_PLAYBACK: &str = "couldn't play track.flac";
const SENTINEL_OPERATION: &str = "scan failed";

#[test]
fn notify_is_a_noop_before_init_then_delivers_after() {
    // Before `init`, `notify` must not panic and simply drops the request.
    notify(ToastKind::PlaybackFailed, "dropped — no consumer yet");

    // First `init` returns the receiver; a second returns None (already set).
    // Defensive `else { return }`: if the global sender were already claimed we
    // can't verify delivery, so skip rather than unwrap. Not reachable in the
    // lib test binary (only this test calls `init`).
    let Some(mut rx) = init() else { return };
    assert!(init().is_none(), "second init must not replace the sender");

    notify(ToastKind::PlaybackFailed, SENTINEL_PLAYBACK);
    notify(ToastKind::OperationFailed, SENTINEL_OPERATION);

    // Both sends are synchronous on an unbounded channel, so our two requests
    // are already queued. Drain everything currently available and keep only our
    // sentinels — a concurrent test's `notify` may have interleaved a foreign
    // request, which we ignore. FIFO guarantees our two keep their relative order.
    let mut seen = Vec::new();
    while let Ok(req) = rx.try_recv() {
        if req.detail == SENTINEL_PLAYBACK || req.detail == SENTINEL_OPERATION {
            seen.push((req.kind, req.detail));
        }
    }

    assert_eq!(
        seen,
        vec![
            (ToastKind::PlaybackFailed, SENTINEL_PLAYBACK.to_owned()),
            (ToastKind::OperationFailed, SENTINEL_OPERATION.to_owned()),
        ],
        "both sentinel toasts must be delivered in order",
    );
}
