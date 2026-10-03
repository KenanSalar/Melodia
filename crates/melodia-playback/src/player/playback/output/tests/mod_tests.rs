//! Tests for the output's own decisions: where the user's volume is applied, how the device's own
//! level is reported, and which backend each platform binds.

use super::{DeviceLevel, voice_gain};

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
/// hardware volume suites would stop compiling in, leaving the rest of the run green. Losing the
/// shared device would send shared output, and every refused claim, back to the system default.
#[cfg(target_os = "windows")]
#[test]
fn windows_binds_the_wasapi_backend_with_every_option_it_offers() {
    let offered = (
        super::EXCLUSIVE_SUPPORTED,
        super::POLLING_SUPPORTED,
        super::HARDWARE_VOLUME_SUPPORTED,
        super::RATE_FALLBACK_SUPPORTED,
        super::SHARED_DEVICE_SUPPORTED,
    );

    assert_eq!(
        offered,
        (true, true, true, true, true),
        "(exclusive, polling, hardware volume, rate fallback, shared device)"
    );
}

/// The Linux twin: a claim, the card's own volume and the rate fallback, but no polling row, since
/// the ALSA writer always blocks on the card and has no event mode to trade for one. Losing the
/// arm would take the ALSA, reservation and card volume suites out of the build with it. No shared
/// device either: a card's `hw:` name would take a shared stream past the sound server.
#[cfg(target_os = "linux")]
#[test]
fn linux_binds_the_alsa_backend_with_every_option_its_writer_has() {
    let offered = (
        super::EXCLUSIVE_SUPPORTED,
        super::POLLING_SUPPORTED,
        super::HARDWARE_VOLUME_SUPPORTED,
        super::RATE_FALLBACK_SUPPORTED,
        super::SHARED_DEVICE_SUPPORTED,
    );

    assert_eq!(
        offered,
        (true, false, true, true, false),
        "(exclusive, polling, hardware volume, rate fallback, shared device)"
    );
}
