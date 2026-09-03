//! Winit `WindowEvent` filter installed by `window_chrome::install`.
//!
//! Seven reasons to subscribe:
//!
//! 1. **`MouseInput { Pressed, Left }`** over the titlebar drag area — an OS-level window
//!    move plus `PreventDefault`, bypassing the TouchArea-grab issue the parent module's
//!    docs describe.
//! 2. **`MouseInput { Pressed, Back | Forward }`** — Mouse-4 / Mouse-5 into the nav
//!    history, gated on no overlay being open so the buttons don't fight its input
//!    context.
//! 3. **`Resized`** — re-read `is_maximized`, so the maximize/restore icon stays in sync
//!    with Win+↑ and window-tile shortcuts that bypass our buttons.
//! 4. **`Focused(bool)`** — mirror OS focus into `Theme.window-focused`, and use the
//!    transition to reconcile the tray-bridge's visibility shadow: raise on focus, ask
//!    the OS about a minimize on focus loss (see [`schedule_minimize_probe`]).
//! 5. **`DroppedFile` / `HoveredFile` / `HoveredFileCancelled`** — Slint 1.16 has no
//!    cross-app `DnD` API, so this is the only path a file drop arrives through.
//! 6. **`MouseWheel`**, for two unrelated `Flickable` defects — see [`route_wheel`].
//!    `CursorMoved` is mirrored for the same arm, a wheel event carrying no position.
//! 7. **`RedrawRequested`** and **`Moved`** — the Slint tick Win32's modal resize-and-move
//!    loop parks winit out of; see [`pump_parked_loop`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::event::{
    ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent,
};
use slint::winit_030::winit::window::Window as WinitWindow;

use crate::{AppWindow, CompositeScroll, PlaylistDetail, PopupHighlight, Queue, Theme};
use melodia_app::state::AppState;

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

/// Run the Slint tick winit's own loop is parked out of.
///
/// Win32 pumps its own message loop for the whole of a resize or move drag, so winit sits inside
/// `DefWindowProcW` and never reaches the wait point that emits `NewEvents` — the one place the
/// backend calls `update_timers_and_animations`. Sizes and paints keep arriving, so the layout
/// tracks the pointer and only the *decisions* stall: every `Timer` and every `changed` handler
/// is frozen until the button comes up, which is the whole responsive layer — the miniplayer
/// swap, `GridColumnsSync`'s column count, each `changed width` mirror.
///
/// One call keeps the drag turning, because the Windows frame throttle is itself a Slint `Timer`:
/// pumping it asks winit for a redraw, whose `WM_PAINT` the modal loop delivers straight back
/// here. It retires on its own once nothing is dirty and the throttle stops itself.
#[cfg(target_os = "windows")]
fn pump_parked_loop() {
    slint::platform::update_timers_and_animations();
}

/// No modal loop off Win32 — every other platform ticks Slint from its own loop iteration.
#[cfg(not(target_os = "windows"))]
fn pump_parked_loop() {}

/// What the `MouseWheel` arm does with one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WheelRoute {
    /// Into the mounted composite view's own scroll math.
    Composite,
    /// Re-send to Slint without the gesture phase, then swallow the original.
    Unphased,
    /// Leave it to Slint.
    Native,
}

/// Decide where one wheel event goes. Two unrelated `Flickable` defects meet here, and the
/// composite view is asked first — it owns its region outright.
///
/// `Composite` is the nested-`ListView`-swallows-the-wheel one, argued at `CompositeScroll`
/// in `globals/shell.slint`. Horizontal-dominant wheel stays native there, that axis
/// belonging to the column pan.
///
/// `Unphased` is the touchpad one. `Flickable` intercepts `TouchPhase::Started`
/// unconditionally and sets its capture flag without checking the delta's axis, so the
/// outermost one under the pointer owns the whole gesture where its `Moved` and
/// `Cancelled` arms check direction first — and only a precision device sends the phase,
/// which is why a plain `TrackList` page scrolls under a wheel and not two fingers.
/// Re-sending unphased leaves the capture flag unset, costing Slint's kinetic fling.
fn route_wheel(
    composite_hovered: bool,
    overlay_open: bool,
    phase: TouchPhase,
    dx: f32,
    dy: f32,
) -> WheelRoute {
    if composite_hovered && !overlay_open && dy != 0.0 && dy.abs() >= dx.abs() {
        return WheelRoute::Composite;
    }
    if matches!(phase, TouchPhase::Started) {
        return WheelRoute::Unphased;
    }
    WheelRoute::Native
}

pub(super) fn install(app: &AppWindow, state: &AppState, drag_hover: Arc<AtomicBool>) {
    let weak = app.as_weak();
    let state = state.clone();
    // Where the pointer is, for the synthetic scroll below.
    let mut cursor_pos = slint::LogicalPosition::default();
    app.window().on_winit_window_event(move |w, event| {
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if drag_hover.load(Ordering::Relaxed) => {
                // Errors are non-fatal: an unsupported backend leaves the cursor where
                // it is, and every desktop platform succeeds.
                let _ = w.with_winit_window(|ww| {
                    if let Err(e) = ww.drag_window() {
                        log::debug!("drag_window unsupported on this platform: {e}");
                    }
                });
                slint::winit_030::EventResult::PreventDefault
            }
            // Above the `Released` cleanup arm because the press is what wants
            // `PreventDefault`. With an overlay up we `Propagate` instead, so Slint
            // keeps its own input semantics — Esc and F already close NP and Queue,
            // and the dialog backdrop catches the dismissing click.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: button @ (MouseButton::Back | MouseButton::Forward),
                ..
            } => {
                let Some(ui) = weak.upgrade() else {
                    return slint::winit_030::EventResult::Propagate;
                };
                if crate::ui::nav_history::overlay_open(&ui) {
                    return slint::winit_030::EventResult::Propagate;
                }
                let going_back = matches!(button, MouseButton::Back);
                crate::ui::nav_history::replay(&state, &ui, going_back);
                slint::winit_030::EventResult::PreventDefault
            }
            // Popup-trigger highlight cleanup. `PopupWindow` has no `closed` callback,
            // and while one is open the dispatcher stops routing mouse events to
            // main-window items, so an AppWindow-level scrim TouchArea can't catch the
            // dismissing click. Release works because the press has already fired Slint's
            // `close-on-click-outside`; popup-internal areas that should *keep* the
            // highlight re-set `PopupHighlight.id` from their own `pointer-event(up)`,
            // which runs after this filter. Both writes are sentinel-gated so a random
            // click doesn't churn the property.
            WindowEvent::MouseInput {
                state: ElementState::Released,
                ..
            } => {
                // Synchronous rather than `upgrade_in_event_loop`: a deferred clear lands
                // after those `pointer-event(up)` handlers and wipes the re-set.
                if let Some(ui) = weak.upgrade() {
                    let ph = ui.global::<PopupHighlight>();
                    if !ph.get_id().is_empty() {
                        ph.set_id("".into());
                    }
                    if ph.get_row_ctx_id() != -1 {
                        ph.set_row_ctx_id(-1);
                    }
                }
                slint::winit_030::EventResult::Propagate
            }
            WindowEvent::Resized(_) => {
                // Into the live mirror while the winit window is still alive: shutdown
                // reads the mirror, `with_winit_window` answering `None` once
                // `app.run()` has returned.
                let maximized = w
                    .with_winit_window(|ww| {
                        geometry::record(ww);
                        // One-shot, on the first (synthetic, post-map) `Resized` only.
                        geometry::ensure_on_screen(ww);
                        ww.is_maximized()
                    })
                    .unwrap_or(false);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let chrome = ui.global::<crate::WindowChrome>();
                    chrome.set_is_maximized(maximized);
                    // The cover tiers size themselves against the window, so this is the edge
                    // that re-derives them — see the handler in `boot::ui_setup`.
                    chrome.invoke_display_changed();
                });
                slint::winit_030::EventResult::Propagate
            }
            // X11/Win32/macOS deliver these; a Wayland client doesn't know its own
            // position, so `Moved` rarely fires there — fine, position restore is a
            // no-op on Wayland anyway.
            WindowEvent::Moved(_) => {
                let _ = w.with_winit_window(geometry::record);
                // A move drag resizes nothing, so the client area is never invalidated and no
                // `WM_PAINT` starts the redraw chain the arm below rides.
                pump_parked_loop();
                slint::winit_030::EventResult::Propagate
            }
            WindowEvent::Focused(focused) => {
                let focused = *focused;
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
                    }
                });
                slint::winit_030::EventResult::Propagate
            }
            // The coalescer batches multi-file drops and re-checks both gates at flush
            // time. Wayland delivery depends on the vendored winit patch.
            WindowEvent::DroppedFile(path) => {
                drop_coalescer::schedule_drop_flush(&state, path.clone());
                // Both banners, whichever gate accepted — otherwise one sticks until the
                // next pointer event.
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.global::<Queue>().set_is_drop_hovered(false);
                    ui.global::<PlaylistDetail>().set_is_drop_hovered(false);
                });
                slint::winit_030::EventResult::Propagate
            }
            // OS-level file hover, driving the queue sheet's banner or Playlist Detail's.
            // Queue takes precedence, and `drop_coalescer`'s flush-side routing applies
            // the same one, so the indicator always names where the drop will land.
            WindowEvent::HoveredFile(_) => {
                let queue_open = drop_coalescer::is_queue_sheet_open();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<Queue>().set_is_drop_hovered(true);
                    // Only when the queue sheet isn't the active drop target.
                    ui.global::<PlaylistDetail>().set_is_drop_hovered(!queue_open);
                });
                slint::winit_030::EventResult::Propagate
            }
            WindowEvent::HoveredFileCancelled => {
                let _ = weak.upgrade_in_event_loop(|ui| {
                    ui.global::<Queue>().set_is_drop_hovered(false);
                    ui.global::<PlaylistDetail>().set_is_drop_hovered(false);
                });
                slint::winit_030::EventResult::Propagate
            }
            // The OS / WM close path — the custom-titlebar button routes through
            // `WindowChrome.on_close_window` instead. Always `PreventDefault` and act
            // explicitly: the loop runs under `run_event_loop_until_quit`, where
            // `CloseRequested` no longer auto-quits. The close-to-tray hide is deferred,
            // running it inside a `WindowEvent` dispatch tripping Slint's "references to
            // the window still exist" warning; `should_hide_to_tray` is false with no
            // tray active, so this can't strand the user.
            WindowEvent::CloseRequested => {
                if crate::ui::shell::tray_bridge::should_hide_to_tray() {
                    if let Err(e) = weak.upgrade_in_event_loop(|ui| {
                        crate::ui::shell::tray_bridge::hide_window(&ui);
                    }) {
                        log::warn!("close-to-tray: schedule hide: {e}");
                    }
                } else if let Err(e) = slint::quit_event_loop() {
                    log::warn!("close-window: quit_event_loop: {e}");
                }
                slint::winit_030::EventResult::PreventDefault
            }
            // The same conversion the Slint backend does for its own copy, so the two
            // can't disagree.
            WindowEvent::CursorMoved { position, .. } => {
                let l = position.to_logical::<f32>(f64::from(w.scale_factor()));
                cursor_pos = slint::LogicalPosition::new(l.x, l.y);
                slint::winit_030::EventResult::Propagate
            }
            // Routing argued at `route_wheel`; the delta conversion mirrors the Slint
            // winit backend exactly.
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(lx, ly) => (lx * 60.0, ly * 60.0),
                    MouseScrollDelta::PixelDelta(p) => {
                        let l = p.to_logical::<f32>(f64::from(w.scale_factor()));
                        (l.x, l.y)
                    }
                };
                let Some(ui) = weak.upgrade() else {
                    return slint::winit_030::EventResult::Propagate;
                };
                let cs = ui.global::<CompositeScroll>();
                // `hovered` can be stale-true where an overlay opened over a composite
                // region with no pointer move, Slint clearing `has-hover` only on the
                // next `MouseEvent::Exit`. Mirrors the Mouse-4/5 arm's own gate.
                let overlay_open = crate::ui::nav_history::overlay_open(&ui);
                match route_wheel(cs.get_hovered(), overlay_open, *phase, dx, dy) {
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
                        slint::winit_030::EventResult::PreventDefault
                    }
                    // `PointerScrolled` lands as a one-shot `TouchPhase::Cancelled`
                    // wheel, the direction-aware arm. Re-sent rather than dropped so
                    // the shortest gesture — one event and a stop — still moves.
                    WheelRoute::Unphased => {
                        let scrolled = slint::platform::WindowEvent::PointerScrolled {
                            position: cursor_pos,
                            delta_x: dx,
                            delta_y: dy,
                        };
                        if let Err(e) = ui.window().try_dispatch_event(scrolled) {
                            log::warn!("touchpad scroll: dispatch: {e}");
                        }
                        slint::winit_030::EventResult::PreventDefault
                    }
                    WheelRoute::Native => slint::winit_030::EventResult::Propagate,
                }
            }
            // The filter runs ahead of Slint's own dispatch, so the last `Resized` has already
            // landed its size and `draw()` has not run yet: a handler firing here sees the frame
            // it is about to be painted into.
            WindowEvent::RedrawRequested => {
                pump_parked_loop();
                slint::winit_030::EventResult::Propagate
            }
            _ => slint::winit_030::EventResult::Propagate,
        }
    });
}

#[cfg(test)]
#[path = "tests/winit_filter_tests.rs"]
mod tests;
