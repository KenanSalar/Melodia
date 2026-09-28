//! Tests for the device-level half of the endpoint volume: which level a slider fraction asks the
//! device for, and which one the device already counts as sitting at. Taking the control, and
//! watching it, need a device, and are tested by hand.

use super::{already_at, level_for};

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

/// A system move comes back as the slider's whole-percent rounding of it, and writing that back
/// would undo a move made meanwhile, so inside half a percent the device is already there. A whole
/// step of Melodia's own slider must never count, or the slider would stop moving the device.
#[test]
fn only_the_sliders_own_rounding_counts_as_the_device_already_being_there() {
    let rows = [
        ("the level itself", 0.37, 0.37, true),
        ("rounded down from a move", 0.37, 0.3749, true),
        ("rounded up from a move", 0.38, 0.3751, true),
        ("at the floor", 0.0, 0.004, true),
        ("just past half a percent above", 0.37, 0.3751, false),
        ("just past half a percent below", 0.38, 0.3749, false),
        ("a whole step", 0.38, 0.37, false),
    ];
    for (what, level, reported, expected) in rows {
        assert_eq!(already_at(level, reported), expected, "{what}");
    }
}
