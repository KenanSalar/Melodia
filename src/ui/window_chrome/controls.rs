//! Wire the `WindowChrome` callback global to OS-level window-control
//! operations and the restart flow.
//!
//! Callback dispatch guarantees the UI thread, so the winit calls are safe directly
//! without an event-loop hop. The persistence work goes to the tokio runtime.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::WindowLevel;

use crate::AppWindow;
use crate::error::AppError;
use crate::services::always_on_top::AlwaysOnTopMethod;
use crate::state::AppState;

pub(super) fn wire(app: &AppWindow, state: &AppState, drag_hover: Arc<AtomicBool>) {
    let chrome = app.global::<crate::WindowChrome>();

    {
        let weak = app.as_weak();
        chrome.on_minimize(move || {
            let Some(ui) = weak.upgrade() else { return };
            // winit directly rather than `slint::Window::set_minimized`, which is
            // gated on an `is_minimized` cache Slint keeps in sync off configure
            // events — and on Wayland `winit::Window::is_minimized` answers `None`,
            // so after a taskbar restore that cache stays stuck at `true` and the
            // next call looks like "no change" and never reaches the OS.
            let _ = ui.window().with_winit_window(|w| w.set_minimized(true));
            // The tray's Show / Hide reads the visibility shadow, so without this
            // drop the next click unmaps an already-minimized surface and restoring
            // takes two. The visualizer's Timer gates on it too.
            crate::ui::shell::tray_bridge::set_window_visible(&ui, false);
        });
    }

    {
        let weak = app.as_weak();
        chrome.on_toggle_maximize(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Read **and** write through winit, mirroring the minimize handler:
            // `slint::Window::set_maximized` only forwards when its cached flag
            // differs, and that cache refreshes on configure events — so any state
            // change not yet round-tripped through the compositor (Win+↑,
            // snap-to-edge, the WM's own gesture) makes the toggle a no-op.
            //
            // `is_maximized()` answers `bool` on every platform, so the
            // `is_minimized` foot-gun doesn't apply to this read.
            let _ = ui.window().with_winit_window(|w| {
                w.set_maximized(!w.is_maximized());
            });
        });
    }

    {
        let weak = app.as_weak();
        chrome.on_close_window(move || {
            // `should_hide_to_tray` is false with no tray, so this can't strand the
            // user with a hidden window. The hide is deferred because calling it
            // inline — inside a winit `WindowEvent` dispatch — trips Slint's
            // "references to the window still exist" warning.
            if crate::ui::shell::tray_bridge::should_hide_to_tray() {
                let weak = weak.clone();
                if let Err(e) = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        crate::ui::shell::tray_bridge::hide_window(&ui);
                    }
                }) {
                    log::warn!("close-to-tray: schedule hide: {e}");
                }
                return;
            }
            // `Window::hide()` is platform-dependent — on some Wayland compositors
            // hiding the only visible window doesn't reliably end the loop, leaving
            // the process alive with no UI. `quit_event_loop()` is explicit.
            if let Err(e) = slint::quit_event_loop() {
                log::warn!("close-window: quit_event_loop: {e}");
            }
        });
    }

    {
        let weak = app.as_weak();
        let state = state.clone();
        chrome.on_toggle_always_on_top(move || {
            let Some(ui) = weak.upgrade() else { return };
            let chrome = ui.global::<crate::WindowChrome>();
            let new = !chrome.get_always_on_top_active();
            let is_native = matches!(state.always_on_top.method, AlwaysOnTopMethod::Native);

            // Optimistic, so icon and tooltip update on the click's own frame;
            // reverted below if the backend errors.
            chrome.set_always_on_top_active(new);

            // The native path is winit-only and UI-thread-only, so it runs before the
            // persistence task. The Linux paths route to the KWin / GNOME backend.
            if is_native {
                let level = if new {
                    WindowLevel::AlwaysOnTop
                } else {
                    WindowLevel::Normal
                };
                let _ = ui.window().with_winit_window(|w| w.set_window_level(level));
            }

            let weak = weak.clone();
            let state_inner = state.clone();
            state.runtime.spawn(async move {
                if let Err(e) = crate::library::window::set_always_on_top(&state_inner, new).await {
                    log::warn!("set_always_on_top: {e}");
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<crate::WindowChrome>().set_always_on_top_active(!new);
                        // Applied through winit above, so roll the OS-level state back
                        // too and keep it in step with the reverted property.
                        if is_native {
                            let revert = if new {
                                WindowLevel::Normal
                            } else {
                                WindowLevel::AlwaysOnTop
                            };
                            let _ = ui.window().with_winit_window(|w| w.set_window_level(revert));
                        }
                    });
                }
            });
        });
    }

    chrome.on_drag_region_hover_changed(move |hovered| {
        drag_hover.store(hovered, Ordering::Relaxed);
    });

    chrome.on_restart_app(restart_toggle(
        app,
        state,
        "use_native_titlebar",
        crate::library::window::set_use_native_titlebar,
    ));
    chrome.on_restart_tray(restart_toggle(
        app,
        state,
        "tray_enabled",
        crate::library::window::set_tray_enabled,
    ));
    chrome.on_restart_backdrop(restart_toggle(
        app,
        state,
        "aurora_backdrop",
        crate::library::window::set_aurora_backdrop,
    ));
}

/// The handler behind a restart-gated setting: persist the value the dialog carries, then ask for
/// the respawn.
///
/// One body for all three because the only thing that differs is which setting is written.
/// `setting` names it for the log line, since `persist` is a plain fn pointer with nothing
/// readable to print.
///
/// **`Dialog.target-id` carries the requested value and must be read before the dispatcher wipes
/// the routing payload.** The respawn is then deferred to `main()`'s exit path: spawning here
/// while the old loop is still wrapping up leaves two windows on screen, the old one unresponsive
/// until its `tracker.wait()` finishes. A refusal there leaves the app running with the new value
/// already on disk.
fn restart_toggle(
    app: &AppWindow,
    state: &AppState,
    setting: &'static str,
    persist: fn(&AppState, bool) -> Result<(), AppError>,
) -> impl Fn() + 'static {
    let weak = app.as_weak();
    let state = state.clone();
    move || {
        let Some(ui) = weak.upgrade() else { return };
        let on = ui.global::<crate::Dialog>().get_target_id() == 1;

        if let Err(e) = persist(&state, on) {
            log::warn!("persist {setting} failed: {e}");
            return;
        }

        super::request_respawn_and_quit();
    }
}
