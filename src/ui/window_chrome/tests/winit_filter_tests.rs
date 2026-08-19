use super::*;
use crate::test_support::{block_body, strip_line_comments};

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

/// Every cover tier sizes its capacity and its decode size against the window, and both are read
/// once after `app.show()`. This arm is the only thing that re-derives them, so without the
/// signal a window maximized after launch mounts more cards than its tier can hold and the
/// overflow paints placeholders until something else moves the visible set. A source walk
/// because the handler at the other end needs a live window and all seven view handles.
#[test]
fn the_resize_arm_retunes_the_cover_tiers() {
    const ARM: &str = "WindowEvent::Resized(_) =>";
    let code = strip_line_comments(include_str!("../winit_filter.rs"));
    let arm = code
        .find(ARM)
        .and_then(|at| code[at..].find('{').map(|rel| at + rel))
        .and_then(|open| block_body(&code, open))
        .unwrap_or_default();

    assert!(!arm.is_empty(), "no `{ARM}` block found — the walk is broken, not the code");
    assert!(
        arm.contains("invoke_display_changed()"),
        "a resize moves every answer the cover tiers derive from the window, so this arm owes \
         the signal that re-derives them:\n{arm}"
    );
}

/// Nothing paints the two properties the arm writes, so without the request no frame is
/// scheduled and the `changed` handler that applies them waits for an unrelated event —
/// every notch landing one notch late (#64). A source walk because scheduling is what a
/// unit test can't reach; `route_wheel` beside it is the half that can.
#[test]
fn the_composite_wheel_arm_asks_for_a_frame() {
    const ARM: &str = "WheelRoute::Composite =>";
    let code = strip_line_comments(include_str!("../winit_filter.rs"));
    let arm = code
        .find(ARM)
        .and_then(|at| code[at..].find('{').map(|rel| at + rel))
        .and_then(|open| block_body(&code, open))
        .unwrap_or_default();

    assert!(!arm.is_empty(), "no `{ARM}` block found — the walk is broken, not the code");
    assert!(
        arm.contains("request_redraw()"),
        "the composite arm writes only properties nothing paints, so it must ask for the \
         frame its `changed` handler runs on:\n{arm}"
    );
}
