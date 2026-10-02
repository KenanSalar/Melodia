//! Tests for what a claim asks a WASAPI endpoint: what each rung is declared as, which layouts may
//! be respelled in the short header, the order a claim asks in, the rungs the rate sweep asks, how
//! its threads share the ladder, and which refusals end a claim. Asking a real device needs one,
//! and is tested by hand; the sweep's threads ask a fake.

use parking_lot::Mutex;
use wasapi::{SampleType, WasapiError};
use windows_core::HRESULT;
use windows_sys::Win32::Media::Audio::{
    AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED,
    AUDCLNT_E_UNSUPPORTED_FORMAT,
};

use super::{
    ANY_LAYOUT, RateQueue, candidates, claim_error, device_refusal, short_header_defined,
    sweep_ladder, wave_format,
};
use crate::player::playback::output::claim::{ClaimError, FallbackReason};
use crate::player::playback::output::encode::DeviceFormat;
use crate::player::playback::output::rates::{LADDER, RateSet};
use crate::player::playback::tests::helpers::shape;
use melodia_audio::player::source::audio::{SampleRate, SourceFormat};

const ENDPOINT: &str = "{0.0.0.00000000}.{a-test-endpoint}";

fn windows_error(code: i32) -> WasapiError {
    WasapiError::Windows(windows_core::Error::from_hresult(HRESULT(code)))
}

/// A device that takes `takes`, refuses to be asked at all at `refuses`, and notes every rate it
/// is asked about, from whichever thread asks.
struct FakeDevice {
    takes: &'static [u32],
    refuses: Option<u32>,
    asked: Mutex<Vec<u32>>,
}

impl FakeDevice {
    fn taking(takes: &'static [u32]) -> Self {
        Self { takes, refuses: None, asked: Mutex::new(Vec::new()) }
    }

    /// A device another application takes as the sweep reaches `rate`.
    fn taken_at(rate: u32) -> Self {
        Self { takes: &[], refuses: Some(rate), asked: Mutex::new(Vec::new()) }
    }

    fn ask(&self, rate: SampleRate) -> Result<bool, ClaimError> {
        self.asked.lock().push(rate.get());
        if self.refuses == Some(rate.get()) {
            return Err(ClaimError::Busy("held by another application".into()));
        }
        Ok(self.takes.contains(&rate.get()))
    }

    /// Every rate asked about, in ladder order whichever thread asked first.
    fn asked(&self) -> Vec<u32> {
        let mut asked = self.asked.lock().clone();
        asked.sort_unstable();
        asked
    }
}

/// `device` swept as a claim sweeps it, every helper asking it too.
fn sweep(device: &FakeDevice) -> Result<RateSet, FallbackReason> {
    sweep_ladder(|rate| device.ask(rate), |queue| queue.drain(|rate| device.ask(rate)))
        .map_err(|e| e.reason())
}

fn offered(rates: &[u32]) -> RateSet {
    rates.iter().copied().collect()
}

#[expect(clippy::panic, reason = "a sweep thread panicking is the case under test")]
fn panics(_: &RateQueue) -> Result<Vec<u32>, ClaimError> {
    panic!("a sweep thread failed");
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

/// Windows defines the short header for 16-bit PCM and for float. A wider integer layout stated in
/// it is what the Realtek HD Audio driver takes and then drains as 32-bit samples, a track playing
/// fast and garbled, so no respelling offers it.
#[test]
fn the_short_header_is_offered_only_for_the_layouts_windows_defines_it_for() {
    let stereo = shape(2, 44_100);
    let rows = [
        (DeviceFormat::S16, true),
        (DeviceFormat::S24Packed, false),
        (DeviceFormat::S24High, false),
        (DeviceFormat::S32, false),
        (DeviceFormat::F32, true),
    ];
    for (format, expected) in rows {
        let offered = wave_format(format, stereo).map(|wave| short_header_defined(&wave));

        assert_eq!(offered, Some(expected), "{format}");
    }
}

/// A mono source is asked for as mono before the device's stereo, each through the ladder less
/// the one rung WASAPI cannot declare.
#[test]
fn a_claim_asks_for_the_source_channel_count_first_and_never_the_lsb_rung() {
    let asked: Vec<(u16, DeviceFormat)> =
        candidates(shape(1, 44_100), SourceFormat { bits: 16, float: false, lossy: false }, 2)
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

    assert_eq!(
        asked,
        [
            (6, DeviceFormat::S32),
            (6, DeviceFormat::F32),
            (6, DeviceFormat::S24Packed),
            (6, DeviceFormat::S24High),
            (6, DeviceFormat::S16),
        ]
    );
}

/// The 17 to 24-bit partition with a step either side of it, 16 being the mono case above. Packed
/// is asked first because it is what many USB DACs offer alone, and the LSB-aligned rung is never
/// asked, WASAPI having no way to declare it.
#[test]
fn a_claim_asks_for_each_integer_width_through_the_rungs_wasapi_declares() {
    use DeviceFormat::{F32, S24High, S24Packed, S32};
    let rows: [(u8, &[DeviceFormat]); 3] =
        [(17, &[S24Packed, S32, S24High]), (24, &[S24Packed, S32, S24High]), (25, &[S32, F32])];
    for (bits, expected) in rows {
        let asked: Vec<DeviceFormat> =
            candidates(shape(2, 96_000), SourceFormat { bits, float: false, lossy: false }, 2)
                .map(|(_, format, _)| format)
                .collect();

        assert_eq!(asked, expected, "a {bits}-bit source");
    }
}

/// A device already as wide as the source has no wider layout to offer, and a mix format reporting
/// no channels at all is a driver's nonsense. Either way the claim still asks at the source's own
/// width rather than asking for nothing.
#[test]
fn a_claim_asks_only_at_the_sources_width_where_the_device_reports_nothing_wider() {
    for device_channels in [2, 0] {
        let asked: Vec<(u16, DeviceFormat)> =
            candidates(shape(2, 48_000), SourceFormat::F32, device_channels)
                .map(|(shape, format, _)| (shape.channels.get(), format))
                .collect();

        assert_eq!(
            asked,
            [
                (2, DeviceFormat::S32),
                (2, DeviceFormat::F32),
                (2, DeviceFormat::S24Packed),
                (2, DeviceFormat::S24High),
                (2, DeviceFormat::S16),
            ],
            "a mix format of {device_channels} channels"
        );
    }
}

/// The offered set answers for every track after the claim's, whatever its format, so the sweep
/// has to ask each rung any of their claims could open in. One it skipped would leave a rate the
/// device takes only in that rung out of the set, and a later track that a fresh claim would play
/// at its own rate would stay converted on the open one.
#[test]
fn the_rate_sweep_asks_every_rung_wasapi_declares() {
    let swept: Vec<DeviceFormat> =
        candidates(shape(2, 96_000), ANY_LAYOUT, 2).map(|(_, format, _)| format).collect();
    let rows = [
        (DeviceFormat::S16, true),
        (DeviceFormat::S24Packed, true),
        (DeviceFormat::S24Low, false),
        (DeviceFormat::S24High, true),
        (DeviceFormat::S32, true),
        (DeviceFormat::F32, true),
    ];
    for (format, expected) in rows {
        assert_eq!(swept.contains(&format), expected, "{format}");
    }
}

/// A rate lost between threads would leave a later track converted that a fresh claim plays at its
/// own rate, and the ladder's two ends are where a queue drops one.
#[test]
fn a_sweep_across_threads_answers_exactly_the_rates_the_device_takes() {
    let device = FakeDevice::taking(&[8_000, 44_100, 48_000, 768_000]);

    let swept = sweep(&device);

    assert_eq!(swept, Ok(offered(&[8_000, 44_100, 48_000, 768_000])));
}

/// A rate the device lacks costs a walk of every layout, so one asked twice pays that twice, and
/// one never asked is a rate the set can't hold.
#[test]
fn a_sweep_across_threads_asks_about_each_rate_once() {
    let device = FakeDevice::taking(&[44_100, 48_000]);

    let _ = sweep(&device);

    assert_eq!(device.asked(), LADDER);
}

/// Each helper answers for the rates it asked about, which the claim's own thread never sees.
#[test]
fn the_rates_every_thread_found_are_in_the_set() {
    let device = FakeDevice::taking(&[44_100]);

    let swept =
        sweep_ladder(|rate| device.ask(rate), |_| Ok(vec![96_000])).map_err(|e| e.reason());

    assert_eq!(swept, Ok(offered(&[44_100, 96_000])));
}

/// A device taken or unplugged part way answers every ask that way, and a set short of the rates
/// left to ask would stand for the session.
#[test]
fn a_refusal_ends_the_sweep_with_it() {
    let device = FakeDevice::taken_at(96_000);

    let swept = sweep(&device);

    assert_eq!(swept, Err(FallbackReason::Busy));
}

/// Every ask after a refusal is a call the device refuses too, so the next thread to reach the
/// queue takes nothing off it.
#[test]
fn after_a_refusal_no_thread_takes_another_rate() {
    let device = FakeDevice::taken_at(16_000);
    let queue = RateQueue::default();
    let _ = queue.drain(|rate| device.ask(rate));

    let _ = queue.drain(|rate| device.ask(rate));

    assert_eq!(device.asked(), [8_000, 11_025, 16_000]);
}

/// A helper that can't reach the device takes nothing off the queue, and the claim's own thread
/// asks what it would have.
#[test]
fn a_helper_that_cant_reach_the_device_leaves_its_rates_to_the_others() {
    let device = FakeDevice::taking(&[44_100, 48_000, 96_000, 192_000]);

    let swept = sweep_ladder(|rate| device.ask(rate), |_| Ok(Vec::new())).map_err(|e| e.reason());

    assert_eq!(swept, Ok(offered(&[44_100, 48_000, 96_000, 192_000])));
}

/// A helper that panics may have taken a rate off the queue without asking about it, so the sweep
/// fails whenever one does rather than keep a set that could be short.
#[test]
fn a_helper_that_panics_fails_the_sweep() {
    let device = FakeDevice::taking(&[44_100, 48_000]);

    let swept = sweep_ladder(|rate| device.ask(rate), panics).map_err(|e| e.reason());

    assert_eq!(swept, Err(FallbackReason::Io));
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

/// The step a refusal arrives at is only context. A device lost as the stream starts has to read as
/// not connected, the one reason the reclaim poll watches for and the only thing that brings the
/// claim back with the device.
#[test]
fn a_device_refusal_keeps_its_reason_whichever_step_it_arrives_at() {
    let rows = [
        (AUDCLNT_E_DEVICE_IN_USE, FallbackReason::Busy),
        (AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, FallbackReason::NotAllowed),
        (AUDCLNT_E_DEVICE_INVALIDATED, FallbackReason::NotConnected),
    ];
    for (code, expected) in rows {
        let refused =
            claim_error("Failed to start the audio device", ENDPOINT, windows_error(code));

        assert_eq!(refused.reason(), expected, "{code:#010x}");
    }
}
