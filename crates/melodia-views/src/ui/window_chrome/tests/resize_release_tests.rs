use super::*;

const SIZE: PhysicalSize<u32> = PhysicalSize::new(800, 600);
const RESIZED: PhysicalSize<u32> = PhysicalSize::new(700, 600);

/// A Wayland resize whose pointer has come back, the configure ending it still to come.
fn awaiting_configure() -> ResizeRelease {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Wayland);
    release.pointer(WindowServer::Wayland);
    release
}

#[test]
fn the_first_size_reading_begins_a_resize() {
    let began = [WindowServer::Wayland, WindowServer::Other]
        .map(|server| ResizeRelease::default().resized(SIZE, server));

    assert_eq!(began, [Transition::Began, Transition::Began]);
}

#[test]
fn a_size_that_keeps_changing_mid_resize_is_no_transition() {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Wayland);

    let transition = release.resized(RESIZED, WindowServer::Wayland);

    assert_eq!(transition, Transition::None);
}

#[test]
fn the_pointer_coming_back_releases_a_resize_off_wayland() {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Other);

    let transition = release.pointer(WindowServer::Other);

    assert_eq!(transition, Transition::Released);
}

/// `KWin` hands the pointer back a loop pass ahead of the configure, and a size picked in that
/// gap is one the configure puts straight back.
#[test]
fn the_pointer_coming_back_to_a_wayland_window_waits_for_the_configure() {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Wayland);

    let transition = release.pointer(WindowServer::Wayland);

    assert_eq!(transition, Transition::AwaitingConfigure);
}

// Whatever size it carries: the wait is what makes it the release, not a repeat.
#[test]
fn the_configure_after_the_pointer_releases_a_wayland_resize() {
    let mut release = awaiting_configure();

    let transition = release.resized(RESIZED, WindowServer::Wayland);

    assert_eq!(transition, Transition::Released);
}

/// A drag let go off the window hands no pointer back, so the configure repeating the drag's last
/// size is the only release there is.
#[test]
fn a_repeated_size_releases_a_wayland_resize_let_go_off_the_window() {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Wayland);

    let transition = release.resized(SIZE, WindowServer::Wayland);

    assert_eq!(transition, Transition::Released);
}

// Elsewhere a repeat is a drag that paused, and the pointer's return is still to come.
#[test]
fn a_repeated_size_off_wayland_is_still_mid_resize() {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Other);

    let transition = release.resized(SIZE, WindowServer::Other);

    assert_eq!(transition, Transition::None);
}

// The compositor sent the configure ahead of the pointer, so none is coming.
#[test]
fn an_overdue_configure_releases_the_resize() {
    let mut release = awaiting_configure();

    let transition = release.configure_overdue();

    assert_eq!(transition, Transition::Released);
}

/// A release keeps the size it ended on, and dragging the window again from there opens on that
/// same size. Read as a repeat, the new resize would release without ever telling the window it
/// began.
#[test]
fn a_released_resize_begins_again_on_the_next_reading() {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Wayland);
    release.resized(SIZE, WindowServer::Wayland);

    let transition = release.resized(SIZE, WindowServer::Wayland);

    assert_eq!(transition, Transition::Began);
}

// Every `CursorMoved` reaches the watch, so this is the arm nearly every event takes.
#[test]
fn pointer_input_with_no_resize_under_way_is_no_transition() {
    let transitions = [WindowServer::Wayland, WindowServer::Other]
        .map(|server| ResizeRelease::default().pointer(server));

    assert_eq!(transitions, [Transition::None, Transition::None]);
}

/// Each answer here restarts the grace timer, so a pointer that kept moving through the wait would
/// hold the release off for as long as it moved.
#[test]
fn pointer_input_while_awaiting_the_configure_is_no_transition() {
    let mut release = awaiting_configure();

    let transition = release.pointer(WindowServer::Wayland);

    assert_eq!(transition, Transition::None);
}

#[test]
fn an_overdue_configure_with_no_resize_is_no_transition() {
    let mut release = ResizeRelease::default();

    let transition = release.configure_overdue();

    assert_eq!(transition, Transition::None);
}

// The pointer hasn't come back, so there is no configure to have been overdue for.
#[test]
fn an_overdue_configure_mid_resize_is_no_transition() {
    let mut release = ResizeRelease::default();
    release.resized(SIZE, WindowServer::Wayland);

    let transition = release.configure_overdue();

    assert_eq!(transition, Transition::None);
}

/// The grace timer's callback asks this before it posts, so a configure that released the resize
/// first leaves the timer nothing to post a second time.
#[test]
fn an_overdue_configure_after_the_release_settles_nothing_twice() {
    let mut release = awaiting_configure();
    release.resized(SIZE, WindowServer::Wayland);

    let transition = release.configure_overdue();

    assert_eq!(transition, Transition::None);
}
