//! Tests for the WASAPI backend's pure half: what each rung is declared as, the order a claim asks
//! in, and which refusals end one. Opening a device needs a device, and is tested by hand.

use wasapi::{SampleType, WasapiError};
use windows_core::HRESULT;
use windows_sys::Win32::Media::Audio::{
    AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED,
    AUDCLNT_E_UNSUPPORTED_FORMAT,
};

use super::{candidates, claim_error, device_refusal, wave_format};
use crate::player::playback::output::claim::{ClaimError, FallbackReason};
use crate::player::playback::output::encode::DeviceFormat;
use crate::player::playback::tests::helpers::shape;
use melodia_audio::player::source::audio::SourceFormat;

const ENDPOINT: &str = "{0.0.0.00000000}.{a-test-endpoint}";

fn windows_error(code: i32) -> WasapiError {
    WasapiError::Windows(windows_core::Error::from_hresult(HRESULT(code)))
}

/// Container bits, valid bits, sample type and bytes per stereo frame, for every rung.
///
/// The frame width is what `encode` has to fill exactly: the writer hands the device one buffer of
/// encoded bytes per event, and a declaration one byte off is a length every write refuses.
#[test]
fn each_rung_is_declared_as_the_layout_encode_writes() {
    let stereo = shape(2, 48_000);
    let rows = [
        (DeviceFormat::S16, Some((16, 16, Some(SampleType::Int), 4))),
        (DeviceFormat::S24Packed, Some((24, 24, Some(SampleType::Int), 6))),
        (DeviceFormat::S24Low, None),
        (DeviceFormat::S24High, Some((32, 24, Some(SampleType::Int), 8))),
        (DeviceFormat::S32, Some((32, 32, Some(SampleType::Int), 8))),
        (DeviceFormat::F32, Some((32, 32, Some(SampleType::Float), 8))),
    ];
    for (format, expected) in rows {
        let declared = wave_format(format, stereo).map(|wave| {
            (
                wave.get_bitspersample(),
                wave.get_validbitspersample(),
                wave.get_subformat().ok(),
                wave.get_blockalign(),
            )
        });

        assert_eq!(declared, expected, "{format}");
    }
}

/// A mono source is asked for as mono before the device's stereo, each through the ladder less
/// the one rung WASAPI cannot declare.
#[test]
fn a_claim_asks_for_the_source_channel_count_first_and_never_the_lsb_rung() {
    let asked: Vec<(u16, DeviceFormat)> =
        candidates(shape(1, 44_100), SourceFormat { bits: 16, float: false }, 2)
            .map(|(shape, format, _)| (shape.channels.get(), format))
            .collect();

    assert_eq!(
        asked,
        [
            (1, DeviceFormat::S16),
            (1, DeviceFormat::S32),
            (1, DeviceFormat::S24Packed),
            (1, DeviceFormat::S24High),
            (2, DeviceFormat::S16),
            (2, DeviceFormat::S32),
            (2, DeviceFormat::S24Packed),
            (2, DeviceFormat::S24High),
        ]
    );
}

/// A source wider than the device is still asked for, at its own width, so the refusal that
/// follows is about the channels rather than a claim that asked for nothing.
#[test]
fn a_source_wider_than_the_device_is_asked_for_at_its_own_width() {
    let asked: Vec<(u16, DeviceFormat)> = candidates(shape(6, 48_000), SourceFormat::F32, 2)
        .map(|(shape, format, _)| (shape.channels.get(), format))
        .collect();

    assert_eq!(asked, [(6, DeviceFormat::S32), (6, DeviceFormat::F32)]);
}

/// The answers that end a claim whatever format it asked in. A device with its exclusive-control
/// box cleared answers every query that way, and read as a refused format the panel would blame
/// the track instead of the setting.
#[test]
fn a_barred_busy_or_vanished_device_ends_the_claim_and_a_refused_format_does_not() {
    let rows = [
        (AUDCLNT_E_DEVICE_IN_USE, Some(FallbackReason::Busy)),
        (AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, Some(FallbackReason::NotAllowed)),
        (AUDCLNT_E_DEVICE_INVALIDATED, Some(FallbackReason::NotConnected)),
        (AUDCLNT_E_UNSUPPORTED_FORMAT, None),
    ];
    for (code, expected) in rows {
        let refusal = device_refusal(windows_error(code), ENDPOINT).ok().map(|e| e.reason());

        assert_eq!(refusal, expected, "{code:#010x}");
    }
}

#[test]
fn any_other_failure_is_io_under_the_callers_context() {
    let context = "Failed to open the audio device";
    let failures = [windows_error(AUDCLNT_E_UNSUPPORTED_FORMAT), WasapiError::ClientNotInit];
    for failure in failures {
        let refused = claim_error(context, ENDPOINT, failure);

        assert!(matches!(refused, ClaimError::Io { context: c, .. } if c == context), "{refused}");
    }
}
