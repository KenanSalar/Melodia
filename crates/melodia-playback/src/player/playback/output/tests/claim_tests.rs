//! Tests for which refused claims a track start asks again, how a refused format is named, and
//! whether an open claim plays the next track without a reopen.

use std::time::Duration;

use melodia_audio::player::source::audio::SourceFormat;

use super::{ClaimError, FallbackReason, claim_serves};
use crate::player::playback::output::encode::DeviceFormat;
use crate::player::playback::output::rates::RateSet;
use crate::player::playback::output::{
    Drive, ExclusiveRequest, ExclusiveTuning, Negotiated, OutputFormat, RateFallback,
};
use crate::player::playback::tests::helpers::shape;

const S16_SOURCE: SourceFormat = SourceFormat { bits: 16, float: false, lossy: false };
const S24_SOURCE: SourceFormat = SourceFormat { bits: 24, float: false, lossy: false };

fn request(format: SourceFormat) -> ExclusiveRequest {
    ExclusiveRequest {
        device: Some("{0.0.0.00000000}.{a-test-endpoint}".to_owned()),
        shape: shape(2, 44_100),
        format,
        tuning: ExclusiveTuning::default(),
        hardware_volume: true,
        rate_fallback: RateFallback::Shared,
    }
}

/// A claim running the device in `format` at the rate [`request`] asks for.
fn running(format: DeviceFormat) -> Negotiated {
    Negotiated {
        device_name: None,
        shape: shape(2, 44_100),
        format: OutputFormat::Exclusive(format),
        fallback: None,
        hardware_volume: true,
        device_level: None,
        offered: None,
        requested_period: None,
        period: None,
    }
}

/// A 16-bit request at `rate` under `fallback`.
fn at(rate: u32, fallback: RateFallback) -> ExclusiveRequest {
    ExclusiveRequest { shape: shape(2, rate), rate_fallback: fallback, ..request(S16_SOURCE) }
}

/// A 16-bit claim running the device at `rate`, having found it offering `offered`.
fn running_at(rate: u32, offered: Option<RateSet>) -> Negotiated {
    Negotiated { shape: shape(2, rate), offered, ..running(DeviceFormat::S16) }
}

/// The rates the UMC22's sweep found on Windows.
fn umc22() -> RateSet {
    [32_000, 44_100, 48_000].into_iter().collect()
}

/// A lossy track decodes to float, and naming it only by width called it "a 32-bit source", which
/// reads as a 32-bit integer file the device should have taken.
#[test]
fn a_format_refusal_names_a_float_source_as_float() {
    let rows = [
        (SourceFormat { bits: 32, float: true, lossy: false }, "a 32-bit float source"),
        (SourceFormat { bits: 32, float: false, lossy: false }, "a 32-bit source"),
        (SourceFormat { bits: 24, float: false, lossy: false }, "a 24-bit source"),
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

/// A 16-bit track after a 24-bit one keeps the 24-bit claim and plays gapless, where the reverse
/// reopens: a narrower container can't hold it. A float source is carried by F32 alone and
/// converted by every integer rung, so any integer claim serves it, and a float claim serves no
/// 16- or 24-bit source.
#[test]
fn a_claim_serves_a_new_format_only_where_it_runs_on_that_format_s_ladder() {
    let rows = [
        (S24_SOURCE, DeviceFormat::S24Packed, S16_SOURCE, true),
        (S24_SOURCE, DeviceFormat::S24High, S16_SOURCE, true),
        (SourceFormat::F32, DeviceFormat::S32, S24_SOURCE, true),
        (S16_SOURCE, DeviceFormat::S16, S24_SOURCE, false),
        (S16_SOURCE, DeviceFormat::S32, SourceFormat::F32, true),
        (SourceFormat::F32, DeviceFormat::F32, SourceFormat::F32, true),
        (S16_SOURCE, DeviceFormat::S16, SourceFormat::F32, true),
        (S24_SOURCE, DeviceFormat::S24Packed, SourceFormat::F32, true),
        (SourceFormat::F32, DeviceFormat::F32, S16_SOURCE, false),
    ];
    for (opened_for, running_in, next, expected) in rows {
        let serves = claim_serves(&request(opened_for), &running(running_in), &request(next));

        assert_eq!(serves, expected, "a {next} track on a {running_in} claim");
    }
}

/// Only the source's format may differ, and its rate where the device already runs that rate.
/// Another device, channel count, rate, pacing or volume route is another claim, whatever the
/// device runs in.
#[test]
fn a_claim_never_serves_a_request_for_another_device_shape_or_route() {
    let opened = request(S24_SOURCE);
    let polled = ExclusiveTuning::new(Duration::from_millis(5), Drive::Polling);
    let changes = [
        ExclusiveRequest { device: None, ..request(S16_SOURCE) },
        ExclusiveRequest { shape: shape(2, 48_000), ..request(S16_SOURCE) },
        ExclusiveRequest { shape: shape(1, 44_100), ..request(S16_SOURCE) },
        ExclusiveRequest { tuning: polled, ..request(S16_SOURCE) },
        ExclusiveRequest { hardware_volume: false, ..request(S16_SOURCE) },
    ];
    for next in changes {
        assert!(!claim_serves(&opened, &running(DeviceFormat::S24Packed), &next), "{next:?}");
    }
}

/// A track at another rate plays on the open claim wherever a fresh claim would run the device
/// where it runs now, and reopens wherever one would move it. One row per answer: the open
/// request's rate, the device's own, a rate the offered set converts to the device's, and the
/// three ways a set answers a reopen.
#[test]
fn a_claim_serves_another_rate_only_where_a_fresh_claim_would_run_the_device_where_it_runs() {
    use RateFallback::Resample;
    let rows = [
        ("the open request's rate", 96_000, 48_000, 96_000, true),
        ("the rate the device runs", 96_000, 48_000, 48_000, true),
        ("a rate the set lacks, converted to the device's", 48_000, 48_000, 96_000, true),
        ("a rate the set lacks, converted to another", 48_000, 48_000, 88_200, false),
        ("a rate the set holds that the device isn't running", 48_000, 48_000, 44_100, false),
        ("a rate off the ladder, which the set can't speak for", 48_000, 48_000, 24_000, false),
    ];
    for (what, opened_for, runs_at, next, expected) in rows {
        let serves = claim_serves(
            &at(opened_for, Resample),
            &running_at(runs_at, Some(umc22())),
            &at(next, Resample),
        );

        assert_eq!(serves, expected, "{what}");
    }
}

/// Where no rates are known, or the claim may not convert, only the open request's rate and the
/// device's own serve. The rate in each row is one the set would convert to the device's.
#[test]
fn a_claim_converts_a_new_rate_only_under_resample_and_with_a_set() {
    let rows = [
        ("no set", RateFallback::Resample, None),
        ("Play Through the System Mixer", RateFallback::Shared, Some(umc22())),
    ];
    for (what, fallback, offered) in rows {
        let serves = claim_serves(
            &at(48_000, fallback),
            &running_at(48_000, offered),
            &at(96_000, fallback),
        );

        assert!(!serves, "{what}");
    }
}
