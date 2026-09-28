//! Tests for where the user's volume is applied: on the voices, or on the device's own control.

use super::voice_gain;

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
