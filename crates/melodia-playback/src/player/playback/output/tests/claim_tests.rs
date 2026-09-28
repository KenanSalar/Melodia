//! Tests for which refused claims a track start asks again.

use super::FallbackReason;

/// A retry can only succeed where the answer can change: the holder lets go, the user allows
/// exclusive control, the device comes back, or a fault clears. A rate, channel count or format
/// the device lacks is refused the same way, and retrying it restarted the shared stream at every
/// skip.
#[test]
fn only_a_refusal_that_can_pass_is_retried() {
    let rows = [
        (FallbackReason::Busy, true),
        (FallbackReason::NotAllowed, true),
        (FallbackReason::Reserved { by: "PipeWire".to_owned() }, true),
        (FallbackReason::NotConnected, true),
        (FallbackReason::Io, true),
        (FallbackReason::RateRefused, false),
        (FallbackReason::ChannelsRefused, false),
        (FallbackReason::FormatRefused, false),
        (FallbackReason::Unsupported, false),
    ];
    for (reason, expected) in rows {
        assert_eq!(reason.may_pass_later(), expected, "{reason:?}");
    }
}
