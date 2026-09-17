use super::*;

const PRESSED_AT: LogicalPosition = LogicalPosition::new(100.0, 10.0);

/// A titlebar press at `t0`, waiting for its second half.
fn after_titlebar_press(t0: Instant) -> DoublePress {
    let mut presses = DoublePress::default();
    presses.press(DragRegion::Titlebar, t0, PRESSED_AT);
    presses
}

#[test]
fn a_first_titlebar_press_moves_the_window() {
    let mut presses = DoublePress::default();

    let action = presses.press(DragRegion::Titlebar, Instant::now(), PRESSED_AT);

    assert_eq!(action, PressAction::Move);
}

#[test]
fn a_second_titlebar_press_in_time_and_place_toggles_maximize() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);

    let action = presses.press(DragRegion::Titlebar, t0 + Duration::from_millis(1), PRESSED_AT);

    assert_eq!(action, PressAction::ToggleMaximize);
}

#[test]
fn a_second_press_on_the_interval_moves_the_window() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);

    let action = presses.press(DragRegion::Titlebar, t0 + DOUBLE_PRESS_INTERVAL, PRESSED_AT);

    assert_eq!(action, PressAction::Move);
}

#[test]
fn a_second_press_just_inside_the_interval_toggles_maximize() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);
    let just_inside = DOUBLE_PRESS_INTERVAL.saturating_sub(Duration::from_millis(1));

    let action = presses.press(DragRegion::Titlebar, t0 + just_inside, PRESSED_AT);

    assert_eq!(action, PressAction::ToggleMaximize);
}

#[test]
fn a_second_press_on_the_slop_radius_moves_the_window() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);
    let on_radius = LogicalPosition::new(PRESSED_AT.x, PRESSED_AT.y + DOUBLE_PRESS_SLOP);

    let action = presses.press(DragRegion::Titlebar, t0, on_radius);

    assert_eq!(action, PressAction::Move);
}

#[test]
fn a_second_press_just_inside_the_slop_radius_toggles_maximize() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);
    let just_inside = LogicalPosition::new(PRESSED_AT.x, PRESSED_AT.y + DOUBLE_PRESS_SLOP - 0.5);

    let action = presses.press(DragRegion::Titlebar, t0, just_inside);

    assert_eq!(action, PressAction::ToggleMaximize);
}

// Inside the slop on each axis alone and past it on the diagonal, so a per-axis check reads this
// as a double where Slint's `ClickState` does not.
#[test]
fn a_press_inside_the_slop_box_but_outside_its_radius_moves_the_window() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);
    let corner = LogicalPosition::new(PRESSED_AT.x + 7.5, PRESSED_AT.y + 7.5);

    let action = presses.press(DragRegion::Titlebar, t0, corner);

    assert_eq!(action, PressAction::Move);
}

/// A completed pair is forgotten, so a triple press toggles once and then moves the window rather
/// than toggling it straight back.
#[test]
fn a_third_press_after_a_double_starts_over() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);
    presses.press(DragRegion::Titlebar, t0 + Duration::from_millis(1), PRESSED_AT);

    let action = presses.press(DragRegion::Titlebar, t0 + Duration::from_millis(2), PRESSED_AT);

    assert_eq!(action, PressAction::Move);
}

// The miniplayer is a window size of its own, and maximizing it only hands the window to the full
// player.
#[test]
fn a_miniplayer_region_never_toggles_maximize() {
    let t0 = Instant::now();
    let mut presses = DoublePress::default();
    presses.press(DragRegion::Miniplayer, t0, PRESSED_AT);

    let action = presses.press(DragRegion::Miniplayer, t0 + Duration::from_millis(1), PRESSED_AT);

    assert_eq!(action, PressAction::Move);
}

/// The pointer rides along with a dragged window, so a click straight after a drag lands inside the
/// slop of the press that started it.
#[test]
fn a_window_move_forgets_the_pending_press() {
    let t0 = Instant::now();
    let mut presses = after_titlebar_press(t0);
    presses.moved();

    let action = presses.press(DragRegion::Titlebar, t0 + Duration::from_millis(1), PRESSED_AT);

    assert_eq!(action, PressAction::Move);
}
