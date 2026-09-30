//! Tests for which refused claims a track start asks again, and how a refused format is named.

use melodia_audio::player::source::audio::SourceFormat;

use super::{ClaimError, FallbackReason};

/// A lossy track decodes to float, and naming it only by width called it "a 32-bit source", which
/// reads as a 32-bit integer file the device should have taken.
#[test]
fn a_format_refusal_names_a_float_source_as_float() {
    let rows = [
        (SourceFormat { bits: 32, float: true }, "a 32-bit float source"),
        (SourceFormat { bits: 32, float: false }, "a 32-bit source"),
        (SourceFormat { bits: 24, float: false }, "a 24-bit source"),
    ];
    for (format, named) in rows {
        let message = ClaimError::FormatRefused { format }.to_string();

        assert_eq!(
            message,
            format!("the device takes none of the formats {named} can be written in"),
            "{format:?}"
        );
    }
}

/// A retry can only succeed where the answer can change: the holder lets go, the user allows
/// exclusive control, or a fault clears. A rate, channel count or format the device lacks is
/// refused the same way, and retrying it restarted the shared stream at every skip. So did a
/// device that wasn't connected, which the reclaim poll asks for once it is listed again.
#[test]
fn a_track_start_retries_only_a_refusal_that_can_pass_and_no_poll_watches() {
    let rows = [
        (FallbackReason::Busy, true),
        (FallbackReason::NotAllowed, true),
        (FallbackReason::Reserved { by: "PipeWire".to_owned() }, true),
        (FallbackReason::NotConnected, false),
        (FallbackReason::Io, true),
        (FallbackReason::RateRefused, false),
        (FallbackReason::ChannelsRefused, false),
        (FallbackReason::FormatRefused, false),
        (FallbackReason::Unsupported, false),
    ];
    for (reason, expected) in rows {
        assert_eq!(reason.retry_at_track_start(), expected, "{reason:?}");
    }
}
