use super::*;

// Every mouse wheel on every platform, and a touchpad on X11 and Win32.
#[test]
fn a_wheel_is_left_to_slint() {
    assert_eq!(route_wheel(false, false, TouchPhase::Moved, 0.0, -60.0), WheelRoute::Native);
    assert_eq!(route_wheel(false, false, TouchPhase::Ended, 0.0, 0.0), WheelRoute::Native);
}

// Only the phase decides. Gating on the delta too would leave the horizontal
// half — a column pan under Search's and Browse's vertical outer scroller —
// captured and ignored exactly as before.
#[test]
fn a_touchpad_gesture_start_is_unphased_on_both_axes() {
    assert_eq!(route_wheel(false, false, TouchPhase::Started, 0.0, -8.0), WheelRoute::Unphased);
    assert_eq!(route_wheel(false, false, TouchPhase::Started, -12.0, 0.0), WheelRoute::Unphased);
}

// A composite view drives its own math off the raw delta and wants the gesture
// whole, phases and all.
#[test]
fn a_composite_region_still_takes_the_gesture_start() {
    assert_eq!(route_wheel(true, false, TouchPhase::Started, 0.0, -8.0), WheelRoute::Composite);
    assert_eq!(route_wheel(true, false, TouchPhase::Moved, 0.0, -60.0), WheelRoute::Composite);
}

// The overlay has scrollers of its own, nested the same way, so a released
// gesture still owes normalizing.
#[test]
fn an_overlay_releases_the_composite_arm_without_releasing_the_gesture() {
    assert_eq!(route_wheel(true, true, TouchPhase::Started, 0.0, -8.0), WheelRoute::Unphased);
    assert_eq!(route_wheel(true, true, TouchPhase::Moved, 0.0, -60.0), WheelRoute::Native);
}

#[test]
fn a_horizontal_wheel_is_not_composite() {
    assert_eq!(route_wheel(true, false, TouchPhase::Moved, -60.0, 0.0), WheelRoute::Native);
    assert_eq!(route_wheel(true, false, TouchPhase::Moved, -60.0, 20.0), WheelRoute::Native);
}
