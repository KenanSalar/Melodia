//! Tests for the output's own decisions: where the user's volume is applied, and whether an open
//! claim plays the next track without a reopen.

use std::time::Duration;

use super::encode::DeviceFormat;
use super::{Drive, ExclusiveRequest, ExclusiveTuning, claim_serves, voice_gain};
use crate::player::playback::tests::helpers::shape;
use melodia_audio::player::source::audio::SourceFormat;

const S16_SOURCE: SourceFormat = SourceFormat { bits: 16, float: false };
const S24_SOURCE: SourceFormat = SourceFormat { bits: 24, float: false };

fn request(format: SourceFormat) -> ExclusiveRequest {
    ExclusiveRequest {
        device: Some("{0.0.0.00000000}.{a-test-endpoint}".to_owned()),
        shape: shape(2, 44_100),
        format,
        tuning: ExclusiveTuning::default(),
        hardware_volume: true,
    }
}

/// With the device's control carrying the volume the voices hand over the samples at unity, which
/// is the whole point, except at zero: that silence stays in the voices, so a mute is instant and
/// needs no switch on the device. Off the device, the voices apply the volume as it is.
#[test]
fn the_voices_run_at_unity_only_while_the_device_carries_an_audible_volume() {
    let rows = [
        (0.5, true, 1.0),
        (0.01, true, 1.0),
        (1.0, true, 1.0),
        (0.0, true, 0.0),
        (0.5, false, 0.5),
        (1.0, false, 1.0),
        (0.0, false, 0.0),
    ];
    for (volume, on_device, expected) in rows {
        let gain = voice_gain(volume, on_device);

        assert_eq!(gain.to_bits(), f64::to_bits(expected), "{volume} on device: {on_device}");
    }
}

/// A 16-bit track after a 24-bit one keeps the 24-bit claim and plays gapless, where the reverse
/// reopens: a narrower container can't hold it. A float source is carried by F32 alone, but its
/// ladder opens on S32, so a claim already in either is the one a fresh claim would land on, and
/// a claim in anything else reopens for it.
#[test]
fn a_claim_serves_a_new_format_only_where_it_runs_on_that_format_s_ladder() {
    let rows = [
        (S24_SOURCE, DeviceFormat::S24Packed, S16_SOURCE, true),
        (S24_SOURCE, DeviceFormat::S24High, S16_SOURCE, true),
        (SourceFormat::F32, DeviceFormat::S32, S24_SOURCE, true),
        (S16_SOURCE, DeviceFormat::S16, S24_SOURCE, false),
        (S16_SOURCE, DeviceFormat::S32, SourceFormat::F32, true),
        (SourceFormat::F32, DeviceFormat::F32, SourceFormat::F32, true),
        (S16_SOURCE, DeviceFormat::S16, SourceFormat::F32, false),
        (S24_SOURCE, DeviceFormat::S24Packed, SourceFormat::F32, false),
        (SourceFormat::F32, DeviceFormat::F32, S16_SOURCE, false),
    ];
    for (opened_for, running_in, next, expected) in rows {
        let serves = claim_serves(&request(opened_for), running_in, &request(next));

        assert_eq!(serves, expected, "a {next} track on a {running_in} claim");
    }
}

/// Only the source's format may differ. Another device, shape, pacing or volume route is another
/// claim, whatever the device runs in.
#[test]
fn a_claim_never_serves_a_request_that_differs_in_more_than_the_format() {
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
        assert!(!claim_serves(&opened, DeviceFormat::S24Packed, &next), "{next:?}");
    }
}
