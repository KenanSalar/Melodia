//! Winit `WindowEvent` filter installed by `window_chrome::install`.
//!
//! Ten reasons to subscribe:
//!
//! 1. **`MouseInput { Pressed, Left }`** over a `ResizeRing` grab or a drag region: an
//!    OS-level resize or window move plus `PreventDefault`, bypassing the TouchArea-grab
//!    issue the parent module's docs describe. The titlebar's double press toggles maximize
//!    instead, Slint never seeing the pair (`drag_region`). `CursorMoved` is mirrored for it, a
//!    press carrying no position.
//! 2. **`MouseInput { Pressed, Back | Forward }`** — Mouse-4 / Mouse-5 into the nav
//!    history, gated on no overlay being open so the buttons don't fight its input
//!    context.
//! 3. **`Resized`** — re-read `is_maximized`, so the maximize/restore icon stays in sync
//!    with Win+↑ and window-tile shortcuts that bypass our buttons. Also the two frame readings:
//!    the step a decoration change costs the client, which the miniplayer's exit edge allows for
//!    once it drops the frame, and on Win32 the invisible borders it keeps transparent so the
//!    window's edges stay put.
//! 4. **`Focused(bool)`** — mirror OS focus into `Theme.window-focused`, and use the
//!    transition to reconcile the tray-bridge's visibility shadow: raise on focus, ask
//!    the OS about a minimize on focus loss (see [`schedule_minimize_probe`]).
//! 5. **`DroppedFile` / `HoveredFile` / `HoveredFileCancelled`** — Slint 1.18's winit backend
//!    delivers no cross-app drop, so this is the only path a file drop arrives through.
//! 6. **`MouseWheel`**, for a nested `ListView` that swallows the wheel at its scroll edge. See
//!    [`route_wheel`].
//! 7. **`RedrawRequested`** and **`Moved`** — the Slint tick Win32's modal resize-and-move
//!    loop parks winit out of; see `parked_loop`.
//! 8. **`ThemeChanged`**, Windows only: a light/dark flip for a theme on its System
//!    variant, re-read by `ui::appearance` rather than taken from the event, whose theme
//!    also folds in high contrast, and a cue for the tray icon to re-read its taskbar.
//! 9. **`Resized`**, **`CursorMoved`**, **`CursorEntered`** and **`MouseInput`** together, for
//!    where a resize begins and where it is let go, which no one event reports. See
//!    `resize_release`.
//! 10. **`MouseInput { Pressed }`**, any button and whoever takes it: whether it landed on the
//!     queue panel, which makes Escape, Ctrl+A and Delete the queue's, and a disarm of what an
//!     in-window drag would carry, which **`Touch { Started }`** makes too, standing every drag
//!     down for that touch. See [`claim_queue_keys`] and [`disarm_drag`].

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;
use slint::winit_030::winit::error::ExternalError;
use slint::winit_030::winit::event::{
    ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use slint::winit_030::winit::window::{ResizeDirection, Window as WinitWindow};
use slint::winit_030::{EventResult, WinitWindowAccessor};

use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{
    AppWindow, CompositeScroll, Drag, DragRegion, PlaylistDetail, Queue, Theme, WindowChrome,
};

use super::drag_region::{DoublePress, PressAction};
#[cfg(target_os = "windows")]
use super::parked_loop;
use super::resize_release::ResizeWatch;
use super::{drop_coalescer, geometry};

/// How long to wait after a focus loss before asking whether it was a minimize.
///
/// The window manager sets the minimized state and drops focus at about the same time
/// and not reliably in that order — X11 publishes `_NET_WM_STATE_HIDDEN` on its own
/// event stream, Win32 sends `WM_ACTIVATE` and `WM_SIZE` independently. Asking a moment
/// later sidesteps the ordering question, for one deferred timer per focus loss.
const MINIMIZE_PROBE_DELAY: Duration = Duration::from_millis(250);

/// Ask the OS whether a focus loss was really a minimize, and drop the visibility shadow
/// if it was — bringing a minimize we didn't drive (the native titlebar's button, a
/// taskbar click, "show desktop") to the same gates our own button reaches.
///
/// `winit::Window::is_minimized` answers `None` on Wayland, the protocol never telling a
/// client it was minimized, so that case is left as it was and `ui::visualizer::pulse`
/// covers it.
fn schedule_minimize_probe(weak: slint::Weak<AppWindow>) {
    slint::Timer::single_shot(MINIMIZE_PROBE_DELAY, move || {
        let Some(ui) = weak.upgrade() else { return };
        let minimized = ui.window().with_winit_window(WinitWindow::is_minimized).flatten();
        if minimized == Some(true) {
            crate::ui::shell::tray_bridge::set_window_visible(&ui, false);
        }
    });
}

/// What the `MouseWheel` arm does with one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WheelRoute {
    /// Into the mounted composite view's own scroll math.
    Composite,
    /// Leave it to Slint.
    Native,
}

/// Decides whether one wheel event belongs to the mounted composite view, whose nested
/// `ListView` would otherwise swallow it at its scroll edge (argued at `CompositeScroll` in
/// `globals/shell.slint`). Horizontal-dominant wheel stays native, that axis belonging to the
/// strips' own scrollers.
///
/// Everything else reaches Slint as winit sent it. A touchpad gesture tracks the fingers and
/// flings only when Slint sees its phases, so nothing here may re-send one.
fn route_wheel(composite_hovered: bool, overlay_open: bool, dx: f32, dy: f32) -> WheelRoute {
    if composite_hovered && !overlay_open && dy != 0.0 && dy.abs() >= dx.abs() {
        return WheelRoute::Composite;
    }
    WheelRoute::Native
}

/// Hands a left press to the OS as a window move or resize. A failure is logged and left at that:
/// macOS answers `NotSupported` for a resize and resizes a borderless window at its own edges, and
/// an unsupported move leaves the window where it is.
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

/// Takes down both drop banners, whichever of the two a drop would have landed in.
fn clear_drop_hover(ui: &AppWindow) {
    ui.global::<Queue>().set_is_drop_hovered(false);
    ui.global::<PlaylistDetail>().set_is_drop_hovered(false);
}

/// What the pointer is over that turns a left press into an OS window operation, each kept in
/// step with Slint hover by a `WindowChrome` callback.
pub(super) struct PressTargets {
    /// Which drag region, if any.
    pub(super) drag_region: Rc<Cell<DragRegion>>,
    /// Over one of `ResizeRing`'s grabs, and which way it resizes.
    pub(super) resize: Rc<Cell<Option<ResizeDirection>>>,
}

pub(super) fn install(app: &AppWindow, state: &AppState, targets: PressTargets) {
    let PressTargets { drag_region, resize } = targets;
    let weak = app.as_weak();
    let state = state.clone();
    // Where the pointer is, for the titlebar's double press below.
    let mut cursor_pos = slint::LogicalPosition::default();
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
                drag_press(w, &weak, &mut double_press, drag_region.get(), cursor_pos)
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: button @ (MouseButton::Back | MouseButton::Forward),
                ..
            } => nav_button(&weak, &state, *button),
            WindowEvent::Resized(_) => on_resized(w, &weak),
            // X11/Win32/macOS deliver these. Wayland never does, a client there not knowing its
            // own position: position restore is a no-op there anyway, but a drag pressed again
            // inside the double-press interval still toggles maximize.
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
            #[cfg(target_os = "windows")]
            WindowEvent::ThemeChanged(_) => {
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.global::<melodia_ui::WindowChrome>().invoke_recheck_system_theme();
                    crate::ui::shell::tray_bridge::refresh_icon();
                });
                EventResult::Propagate
            }
            WindowEvent::DroppedFile(path) => on_dropped_file(&state, &weak, path.clone()),
            WindowEvent::HoveredFile(_) => on_hovered_file(&weak),
            WindowEvent::HoveredFileCancelled => {
                let _ = weak.upgrade_in_event_loop(|ui| clear_drop_hover(&ui));
                EventResult::Propagate
            }
            WindowEvent::CloseRequested => on_close_requested(&weak),
            // The same conversion the Slint backend does for its own copy, so the two
            // can't disagree.
            WindowEvent::CursorMoved { position, .. } => {
                let l = position.to_logical::<f32>(f64::from(w.scale_factor()));
                cursor_pos = slint::LogicalPosition::new(l.x, l.y);
                EventResult::Propagate
            }
            WindowEvent::MouseWheel { delta, .. } => on_mouse_wheel(w, &weak, *delta),
            // The filter runs ahead of Slint's own dispatch, so the last `Resized` has already
            // landed its size and `draw()` has not run yet: a handler firing here sees the frame
            // it is about to be painted into.
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

/// What a press came from, which decides whether it can start a drag.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pointer {
    Mouse,
    Touch,
}

/// Empties what an in-window drag would carry, ahead of Slint's dispatch, so only the row this
/// press lands on can arm it. A list's `DragArea` starts a drag from any press it contains, a gap
/// between rows or a row's own button included, and no element sees all of those, so without this
/// such a drag would carry the last row clicked. Here for the reason [`claim_queue_keys`] is.
///
/// It also says whether the press is a touch, and every `DragSource` stands down for one, so the
/// list under a finger pans rather than the two fighting over the gesture.
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
    cursor_pos: slint::LogicalPosition,
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

/// Mouse-4 / Mouse-5 into the nav history. With an overlay up we `Propagate` instead, so Slint
/// keeps its own input semantics — Esc and F already close Now Playing, and the dialog backdrop
/// catches the dismissing click.
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
    // Into the live mirror while the winit window is still alive: shutdown
    // reads the mirror, `with_winit_window` answering `None` once
    // the event loop has returned.
    let (maximized, frame, margins) = w
        .with_winit_window(|ww| {
            let reading = geometry::WindowReading::take(ww);
            geometry::record(ww, reading);
            // One-shot, on the first (synthetic, post-map) `Resized` only.
            geometry::ensure_on_screen(ww, reading);
            (
                reading.is_maximized(),
                geometry::frame_allowance(reading),
                geometry::frame_margins(ww, reading),
            )
        })
        .unwrap_or((false, None, None));
    // Synchronous, where everything below is posted: this filter runs ahead of Slint's
    // own handling, so the size that arrives with this event is the first thing
    // `MiniPlayerSwitch` measures against the exit edge. Posted, the allowance for the
    // frame it has just dropped lands a beat after the decision that needed it, and the
    // miniplayer swaps straight back out of the size the drop grew it to.
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
        // The cover tiers size themselves against the window, so this is the edge
        // that re-derives them — see the handler in `boot::ui_setup`.
        chrome.invoke_display_changed();
    });
    EventResult::Propagate
}

fn on_focus(weak: &slint::Weak<AppWindow>, focused: bool) -> EventResult {
    // `Focused(false)` alone is ambiguous, so ask the OS which — but only
    // while we still believe the window is up: a tray hide and our own
    // minimize button both lower the shadow first, and on X11 the answer
    // costs a round-trip on the UI thread.
    if !focused && crate::ui::shell::tray_bridge::is_window_visible() {
        schedule_minimize_probe(weak.clone());
    }
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<Theme>().set_window_focused(focused);
        // Focus implies the surface is visible, covering the taskbar-restore
        // path: the WM un-minimizes without going through any of our
        // callbacks, so the shadow would stay stuck `false`.
        if focused {
            crate::ui::shell::tray_bridge::set_window_visible(&ui, true);
            // The theme first: its repaint moves the palette the border's neutral is
            // mixed from.
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

/// The coalescer batches multi-file drops and re-checks both gates at flush
/// time. Wayland delivery depends on the vendored winit patch.
fn on_dropped_file(
    state: &AppState,
    weak: &slint::Weak<AppWindow>,
    path: std::path::PathBuf,
) -> EventResult {
    drop_coalescer::schedule_drop_flush(state, path);
    // Both banners, whichever gate accepted — otherwise one sticks until the
    // next pointer event.
    let _ = weak.upgrade_in_event_loop(|ui| clear_drop_hover(&ui));
    EventResult::Propagate
}

/// OS-level file hover, driving the queue panel's banner or Playlist Detail's.
/// Queue takes precedence, and `drop_coalescer`'s flush-side routing applies
/// the same one, so the indicator always names where the drop will land.
fn on_hovered_file(weak: &slint::Weak<AppWindow>) -> EventResult {
    let queue_shown = drop_coalescer::is_queue_panel_shown();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<Queue>().set_is_drop_hovered(true);
        // Only when the queue panel isn't the active drop target.
        ui.global::<PlaylistDetail>().set_is_drop_hovered(!queue_shown);
    });
    EventResult::Propagate
}

/// The OS / WM close path — the custom-titlebar button routes through
/// `WindowChrome.on_close_window` instead. Always `PreventDefault` and act
/// explicitly: the loop runs under `run_event_loop_until_quit`, where
/// `CloseRequested` no longer auto-quits. The close-to-tray hide is deferred,
/// running it inside a `WindowEvent` dispatch tripping Slint's "references to
/// the window still exist" warning; `should_hide_to_tray` is false with no
/// tray active, so this can't strand the user.
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

/// Routing argued at [`route_wheel`]; the delta conversion mirrors the Slint
/// winit backend exactly.
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
    // `hovered` can be stale-true where an overlay opened over a composite
    // region with no pointer move, Slint clearing `has-hover` only on the
    // next `MouseEvent::Exit`. Mirrors the Mouse-4/5 arm's own gate.
    let overlay_open = crate::ui::nav_history::overlay_open(&ui);
    match route_wheel(cs.get_hovered(), overlay_open, dx, dy) {
        WheelRoute::Composite => {
            // Accumulate rather than overwrite: the `changed wheel-tick`
            // handler fires at most once per loop iteration, so several
            // events before a re-eval would collapse to the last delta and
            // under-scroll a fast flick. The view zeroes `wheel-dy` after
            // applying, so this only sums within one un-applied frame.
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
