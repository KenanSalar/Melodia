//! Tests for the output's own decisions: where the user's volume is applied, whether an open claim
//! plays the next track without a reopen, and how the device's own level is reported.

use std::time::Duration;

use super::encode::DeviceFormat;
use super::{
    DeviceLevel, Drive, ExclusiveRequest, ExclusiveTuning, RateFallback, claim_serves, voice_gain,
};
use crate::player::playback::tests::helpers::shape;
use melodia_audio::player::source::audio::SourceFormat;

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

/// The row reads a percentage off the device, so the fraction the system's slider shows rounds to
/// its nearest whole one, and anything past either end is held to it.
#[test]
fn a_device_level_is_the_nearest_whole_percent_the_system_shows() {
    let rows = [
        (0.7058, 71),
        (0.994, 99),
        (0.996, 100),
        (0.0, 0),
        (1.0, 100),
        (1.2, 100),
        (-0.1, 0),
        (f32::NAN, 0),
    ];
    for (level, percent) in rows {
        assert_eq!(DeviceLevel::new(level, false).percent, percent, "{level}");
    }
}

/// Every option the Output card offers on Windows is the WASAPI backend's answer. Were the
/// `cfg_select!` arm to stop binding it, every claim would fall back to shared and the WASAPI and
/// hardware volume suites would stop compiling in, leaving the rest of the run green.
#[cfg(target_os = "windows")]
#[test]
fn windows_binds_the_wasapi_backend_with_every_option_it_offers() {
    let offered =
        (super::EXCLUSIVE_SUPPORTED, super::POLLING_SUPPORTED, super::HARDWARE_VOLUME_SUPPORTED);

    assert_eq!(offered, (true, true, true), "(exclusive, polling, hardware volume)");
}

/// The Linux twin: a claim and the card's own volume, but no polling row, since the ALSA writer
/// always blocks on the card and has no event mode to trade for one. Losing the arm would take the
/// ALSA, reservation and card volume suites out of the build with it.
#[cfg(target_os = "linux")]
#[test]
fn linux_binds_the_alsa_backend_with_every_option_its_writer_has() {
    let offered =
        (super::EXCLUSIVE_SUPPORTED, super::POLLING_SUPPORTED, super::HARDWARE_VOLUME_SUPPORTED);

    assert_eq!(offered, (true, false, true), "(exclusive, polling, hardware volume)");
}
