//! The winit `WindowEvent` filter, for what Slint sees too late or not at all: a left press the OS
//! has to take over as a move or resize, a press a child took first, Mouse-4/5, the vendored fork's
//! file drags, a wheel a nested `ListView` would swallow, and the tick a Win32 drag parks. Each arm
//! argues itself at its handler.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::winit_030::winit::dpi::PhysicalPosition;
use slint::winit_030::winit::error::ExternalError;
use slint::winit_030::winit::event::{
    ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use slint::winit_030::winit::window::{ResizeDirection, Window as WinitWindow};
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition};

use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{
    AppWindow, CompositeScroll, Drag, DragRegion, FileDrag, FileDropTarget, Queue, Theme,
    WindowChrome,
};

use super::drag_region::{DoublePress, PressAction};
use super::file_drop::{self, DropTargets};
use super::geometry;
#[cfg(target_os = "windows")]
use super::parked_loop;
use super::pointer;
use super::resize_release::ResizeWatch;

/// How long after a focus loss to ask whether it was a minimize: the WM sets the state and drops
/// focus in no reliable order (X11 publishes `_NET_WM_STATE_HIDDEN` on its own stream, Win32 sends
/// `WM_ACTIVATE` and `WM_SIZE` independently).
const MINIMIZE_PROBE_DELAY: Duration = Duration::from_millis(250);

/// Drops the visibility shadow if a focus loss was a minimize Melodia didn't drive (the native
/// titlebar's button, the taskbar, "show desktop"). Wayland never tells a client it was minimized,
/// so there `ui::visualizer::pulse` covers it.
fn schedule_minimize_probe(weak: slint::Weak<AppWindow>) {
    slint::Timer::single_shot(MINIMIZE_PROBE_DELAY, move || {
        let Some(ui) = weak.upgrade() else { return };
        let minimized = ui.window().with_winit_window(WinitWindow::is_minimized).flatten();
        if minimized == Some(true) {
            crate::ui::shell::tray_bridge::set_window_visible(&ui, false);
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WheelRoute {
    Composite,
    Native,
}

/// Sends vertical-dominant wheel over the mounted composite view to its own scroll math, its nested
/// `ListView` otherwise swallowing it at the scroll edge (argued at `CompositeScroll` in
/// `globals/shell.slint`). The rest reaches Slint as winit sent it: a touchpad gesture flings only
/// when Slint sees its phases, so nothing here may re-send one.
fn route_wheel(composite_hovered: bool, overlay_open: bool, dx: f32, dy: f32) -> WheelRoute {
    if composite_hovered && !overlay_open && dy != 0.0 && dy.abs() >= dx.abs() {
        return WheelRoute::Composite;
    }
    WheelRoute::Native
}

/// Hands a left press to the OS as a window move or resize. A failure is only logged: macOS answers
/// `NotSupported` for a resize and resizes a borderless window at its own edges.
fn start_os_drag(
    w: &slint::Window,
    what: &str,
    op: impl FnOnce(&WinitWindow) -> Result<(), ExternalError>,
) {
    let _ = w.with_winit_window(|ww| {
        if let Err(e) = op(ww) {
            log::debug!("{what} unsupported on this platform: {}", describe(&e));
        }
    });
}

/// The Slint backend's own conversion, so the two can't disagree.
fn to_logical(w: &slint::Window, position: PhysicalPosition<f64>) -> LogicalPosition {
    let l = position.to_logical::<f32>(f64::from(w.scale_factor()));
    LogicalPosition::new(l.x, l.y)
}

/// What the pointer is over that turns a left press into an OS window operation, each kept in
/// step with Slint hover by a `WindowChrome` callback.
pub(super) struct PressTargets {
    pub(super) drag_region: Rc<Cell<DragRegion>>,
    pub(super) resize: Rc<Cell<Option<ResizeDirection>>>,
}

pub(super) fn install(app: &AppWindow, state: &AppState, targets: PressTargets) {
    let PressTargets { drag_region, resize } = targets;
    let weak = app.as_weak();
    let state = state.clone();
    let mut double_press = DoublePress::default();
    let resize_watch = ResizeWatch::new(app.as_weak());
    app.window().on_winit_window_event(move |w, event| {
        match event {
            WindowEvent::Resized(size) => resize_watch.resized(w, *size),
            WindowEvent::CursorMoved { .. }
            | WindowEvent::CursorEntered { .. }
            | WindowEvent::MouseInput { .. } => resize_watch.pointer(w),
            _ => {}
        }
        if let WindowEvent::MouseInput { state: ElementState::Pressed, .. } = event {
            claim_queue_keys(&weak);
            disarm_drag(&weak, Pointer::Mouse);
        }
        // Slint takes a first touch as a left press, so it disarms too, and stands the drags down
        // until a mouse presses again. The queue's claim stays a mouse press's: it reads the
        // pointer's hover, which no touch has ahead of it.
        if let WindowEvent::Touch(touch) = event
            && touch.phase == TouchPhase::Started
        {
            disarm_drag(&weak, Pointer::Touch);
            pointer::leave();
        }
        match event {
            // Ahead of the drag arm: the miniplayer's drag region runs right up to the edge, and its
            // hover can still read true for a tick after the pointer reaches a grab.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if let Some(direction) = resize.get() => {
                start_os_drag(w, "drag_resize_window", |ww| ww.drag_resize_window(direction));
                EventResult::PreventDefault
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if drag_region.get() != DragRegion::None => {
                // A press carries no position, so the double press reads the last move's.
                let at = pointer::resting().unwrap_or_default();
                drag_press(w, &weak, &mut double_press, drag_region.get(), at)
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: button @ (MouseButton::Back | MouseButton::Forward),
                ..
            } => nav_button(&weak, &state, *button),
            WindowEvent::Resized(_) => on_resized(w, &weak),
            // Never on Wayland, so there a press straight after a move drag can still double it.
            WindowEvent::Moved(_) => {
                double_press.moved();
                let _ = w.with_winit_window(|ww| {
                    geometry::record(ww, geometry::WindowReading::take(ww));
                });
                // A move drag resizes nothing, so the client area is never invalidated and no
                // `WM_PAINT` starts the redraw chain the arm below rides.
                #[cfg(target_os = "windows")]
                parked_loop::pump();
                EventResult::Propagate
            }
            WindowEvent::Focused(focused) => on_focus(&weak, *focused),
            // Re-read by `ui::appearance` rather than taken from the event, whose theme folds in
            // high contrast.
            #[cfg(target_os = "windows")]
            WindowEvent::ThemeChanged(_) => {
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.global::<melodia_ui::WindowChrome>().invoke_recheck_system_theme();
                    crate::ui::shell::tray_bridge::refresh_icon();
                });
                EventResult::Propagate
            }
            WindowEvent::DragEntered { position, .. } | WindowEvent::DragMoved { position } => {
                on_file_drag(&weak, to_logical(w, *position))
            }
            WindowEvent::DragDropped { paths, position } => {
                on_file_drop(&state, &weak, paths, to_logical(w, *position))
            }
            WindowEvent::DragLeft { .. } => {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<FileDrag>().set_target(FileDropTarget::None);
                }
                EventResult::Propagate
            }
            WindowEvent::CloseRequested => on_close_requested(&weak),
            WindowEvent::CursorMoved { position, .. } => {
                pointer::rest_at(to_logical(w, *position));
                EventResult::Propagate
            }
            WindowEvent::CursorLeft { .. } => {
                pointer::leave();
                EventResult::Propagate
            }
            WindowEvent::MouseWheel { delta, .. } => on_mouse_wheel(w, &weak, *delta),
            // Ahead of Slint's dispatch, the last `Resized` has landed its size and `draw()` hasn't
            // run, so a handler firing here sees the frame it's about to be painted into.
            WindowEvent::RedrawRequested => {
                #[cfg(target_os = "windows")]
                parked_loop::pump();
                EventResult::Propagate
            }
            _ => EventResult::Propagate,
        }
    });
}

/// Gives the queue Escape, Ctrl+A and Delete when a press lands on its panel, and takes them back
/// when one lands anywhere else. Read here because no element sees a press a child took first, and
/// the panel is all children. Only a shown panel claims: `pointer-in` moves with the pointer, not
/// the panel, so one that eased away under a still pointer would take the next press.
fn claim_queue_keys(weak: &slint::Weak<AppWindow>) {
    let Some(ui) = weak.upgrade() else { return };
    let queue = ui.global::<Queue>();
    queue.set_keys_owned(queue.get_shown() && queue.get_pointer_in());
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pointer {
    Mouse,
    Touch,
}

/// Empties what an in-window drag would carry, ahead of Slint's dispatch, so only the row this
/// press lands on can arm it: a list's `DragArea` starts a drag from any press inside it, a gap or
/// a row's button included, and without this would carry the last row clicked. A touch also stands
/// every `DragSource` down, so the list under a finger pans rather than fighting it.
fn disarm_drag(weak: &slint::Weak<AppWindow>, pointer: Pointer) {
    let Some(ui) = weak.upgrade() else { return };
    let drag = ui.global::<Drag>();
    drag.set_touching(pointer == Pointer::Touch);
    drag.invoke_disarm();
}

/// A left press over a drag region: a window move, or the titlebar's double press.
fn drag_press(
    w: &slint::Window,
    weak: &slint::Weak<AppWindow>,
    double_press: &mut DoublePress,
    region: DragRegion,
    cursor_pos: LogicalPosition,
) -> EventResult {
    match double_press.press(region, Instant::now(), cursor_pos) {
        PressAction::Move => start_os_drag(w, "drag_window", WinitWindow::drag_window),
        PressAction::ToggleMaximize => {
            if let Some(ui) = weak.upgrade() {
                ui.global::<WindowChrome>().invoke_toggle_maximize();
            }
        }
    }
    EventResult::PreventDefault
}

/// Mouse-4/5 into the nav history, left to Slint while an overlay is up: Esc and F already close
/// Now Playing, and the dialog backdrop takes the dismissing click.
fn nav_button(weak: &slint::Weak<AppWindow>, state: &AppState, button: MouseButton) -> EventResult {
    let Some(ui) = weak.upgrade() else {
        return EventResult::Propagate;
    };
    if crate::ui::nav_history::overlay_open(&ui) {
        return EventResult::Propagate;
    }
    let going_back = matches!(button, MouseButton::Back);
    crate::ui::nav_history::replay(state, &ui, going_back);
    EventResult::PreventDefault
}

fn on_resized(w: &slint::Window, weak: &slint::Weak<AppWindow>) -> EventResult {
    let (maximized, frame, margins) = w
        .with_winit_window(|ww| {
            let reading = geometry::WindowReading::take(ww);
            geometry::record(ww, reading);
            geometry::ensure_on_screen(ww, reading);
            (
                reading.is_maximized(),
                geometry::frame_allowance(reading),
                geometry::frame_margins(ww, reading),
            )
        })
        .unwrap_or((false, None, None));
    // Synchronous, unlike the rest: the filter runs ahead of Slint, so this size is the first
    // `MiniPlayerSwitch` measures against the exit edge. Posted, the allowance lands after the
    // decision that needed it and the miniplayer swaps straight back out.
    if let Some(frame) = frame
        && let Some(ui) = weak.upgrade()
    {
        let chrome = ui.global::<melodia_ui::WindowChrome>();
        chrome.set_frame_allowance_w(frame.width);
        chrome.set_frame_allowance_h(frame.height);
    }
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let chrome = ui.global::<melodia_ui::WindowChrome>();
        chrome.set_is_maximized(maximized);
        if let Some(margins) = margins {
            chrome.set_frame_margin_left(margins.left);
            chrome.set_frame_margin_right(margins.right);
            chrome.set_frame_margin_bottom(margins.bottom);
        }
        // The cover tiers size against the window (`boot::ui_setup`'s handler).
        chrome.invoke_display_changed();
    });
    EventResult::Propagate
}

fn on_focus(weak: &slint::Weak<AppWindow>, focused: bool) -> EventResult {
    // Only while the shadow says the window is up: a tray hide and our minimize button lower it
    // first, and on X11 the probe is a round trip on the UI thread.
    if !focused && crate::ui::shell::tray_bridge::is_window_visible() {
        schedule_minimize_probe(weak.clone());
    }
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<Theme>().set_window_focused(focused);
        // A taskbar restore un-minimizes through none of our callbacks.
        if focused {
            crate::ui::shell::tray_bridge::set_window_visible(&ui, true);
            // The theme first: its repaint moves the palette the border's neutral is mixed from.
            #[cfg(target_os = "windows")]
            {
                ui.global::<melodia_ui::WindowChrome>().invoke_recheck_system_theme();
                crate::ui::appearance::window_border::refresh_system_color(&ui);
            }
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            crate::ui::shell::tray_bridge::refresh_icon();
        }
    });
    EventResult::Propagate
}

/// A file held over the window raises the banner of whichever target letting go would land in.
fn on_file_drag(weak: &slint::Weak<AppWindow>, point: LogicalPosition) -> EventResult {
    if let Some(ui) = weak.upgrade() {
        let route = file_drop::route(point, &DropTargets::read(&ui));
        ui.global::<FileDrag>().set_target(route.target());
    }
    EventResult::Propagate
}

/// Files let go of land where the banner over them said, routed again at the release point.
fn on_file_drop(
    state: &AppState,
    weak: &slint::Weak<AppWindow>,
    paths: &[PathBuf],
    point: LogicalPosition,
) -> EventResult {
    let Some(ui) = weak.upgrade() else {
        return EventResult::Propagate;
    };
    let route = file_drop::route(point, &DropTargets::read(&ui));
    ui.global::<FileDrag>().set_target(FileDropTarget::None);
    file_drop::import(state, route, paths);
    EventResult::Propagate
}

/// The OS's close; the custom titlebar's button goes through `WindowChrome.close-window` instead.
/// Under `run_event_loop_until_quit` a close no longer quits by itself. The tray hide is posted:
/// run inside a `WindowEvent` dispatch, it trips Slint's "references to the window still exist".
fn on_close_requested(weak: &slint::Weak<AppWindow>) -> EventResult {
    if crate::ui::shell::tray_bridge::should_hide_to_tray() {
        if let Err(e) = weak.upgrade_in_event_loop(|ui| {
            crate::ui::shell::tray_bridge::hide_window(&ui);
        }) {
            log::warn!("close-to-tray: schedule hide: {}", describe(&e));
        }
    } else if let Err(e) = slint::quit_event_loop() {
        log::warn!("close-window: quit_event_loop: {}", describe(&e));
    }
    EventResult::PreventDefault
}

/// The delta conversion is the Slint backend's, so a composite scroll moves as a native one does.
fn on_mouse_wheel(
    w: &slint::Window,
    weak: &slint::Weak<AppWindow>,
    delta: MouseScrollDelta,
) -> EventResult {
    let (dx, dy) = match delta {
        MouseScrollDelta::LineDelta(lx, ly) => (lx * 60.0, ly * 60.0),
        MouseScrollDelta::PixelDelta(p) => {
            let l = p.to_logical::<f32>(f64::from(w.scale_factor()));
            (l.x, l.y)
        }
    };
    let Some(ui) = weak.upgrade() else {
        return EventResult::Propagate;
    };
    let cs = ui.global::<CompositeScroll>();
    // `hovered` stays true under an overlay opened without a pointer move, Slint clearing
    // `has-hover` only on the next exit.
    let overlay_open = crate::ui::nav_history::overlay_open(&ui);
    match route_wheel(cs.get_hovered(), overlay_open, dx, dy) {
        WheelRoute::Composite => {
            // Summed, not overwritten: the `changed wheel-tick` handler runs at most once per loop
            // pass, so a fast flick would collapse to its last delta. The view zeroes `wheel-dy`.
            cs.set_wheel_dy(cs.get_wheel_dy() + dy);
            cs.set_wheel_tick(cs.get_wheel_tick().wrapping_add(1));
            // Nothing paints either property, so nothing else asks for the
            // frame whose `new_events` runs the handler that applies them.
            ui.window().request_redraw();
            EventResult::PreventDefault
        }
        WheelRoute::Native => EventResult::Propagate,
    }
}

#[cfg(test)]
#[path = "tests/winit_filter_tests.rs"]
mod tests;
