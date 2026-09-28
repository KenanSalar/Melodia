//! Tests for the device-level half of the endpoint volume: which level a slider fraction asks the
//! device for. Taking the control, and watching it, need a device, and are tested by hand.

use super::level_for;

/// The fraction goes over as it is, so the percentage the system shows for the device is the one
/// Melodia's slider shows. Anything outside `0..=1` is held to it, since the device refuses a level
/// past its ends rather than taking the nearest.
#[test]
fn the_slider_fraction_is_the_level_the_system_shows() {
    let rows = [
        (0.5, 0.5),
        (0.39, 0.39),
        (1.0, 1.0),
        (0.0, 0.0),
        (-0.25, 0.0),
        (1.5, 1.0),
        (f64::NAN, 0.0),
    ];
    for (volume, expected) in rows {
        let level: f32 = level_for(volume);

        assert_eq!(level.to_bits(), f32::to_bits(expected), "{volume}");
    }
}
