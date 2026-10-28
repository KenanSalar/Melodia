//! The `WindowChrome` callbacks: the window controls, through winit, and the restart flow.

use std::sync::Arc;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::WindowLevel;

use melodia_app::state::AppState;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, describe};
use melodia_platform::services::platform::always_on_top::AlwaysOnTopMethod;
use melodia_ui::AppWindow;

pub(super) fn wire(app: &AppWindow, state: &AppState) {
    let chrome = app.global::<melodia_ui::WindowChrome>();

    {
        let weak = app.as_weak();
        chrome.on_minimize(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Not `slint::Window::set_minimized`, gated on an `is_minimized` cache that Wayland's
            // `None` leaves stuck at `true` after a taskbar restore, so the call never reaches the OS.
            let _ = ui.window().with_winit_window(|w| w.set_minimized(true));
            // The tray's Show/Hide and the visualizer's Timer read the shadow; left up, restoring
            // takes two clicks.
            crate::ui::shell::tray_bridge::set_window_visible(&ui, false);
        });
    }

    {
        let weak = app.as_weak();
        chrome.on_toggle_maximize(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Read and written through winit: Slint forwards `set_maximized` only when its cached
            // flag differs, so a Win+↑ or snap not yet round-tripped makes the toggle a no-op.
            let _ = ui.window().with_winit_window(|w| {
                w.set_maximized(!w.is_maximized());
            });
        });
    }

    {
        let weak = app.as_weak();
        chrome.on_restore_full_player(move || {
            let Some(ui) = weak.upgrade() else { return };
            super::geometry::restore_full_player(&ui);
        });
    }

    {
        let weak = app.as_weak();
        chrome.on_close_window(move || {
            // Posted: inside a winit `WindowEvent` dispatch the hide trips Slint's "references to
            // the window still exist".
            if crate::ui::shell::tray_bridge::should_hide_to_tray() {
                let weak = weak.clone();
                if let Err(e) = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        crate::ui::shell::tray_bridge::hide_window(&ui);
                    }
                }) {
                    log::warn!("close-to-tray: schedule hide: {}", describe(&e));
                }
                return;
            }
            // Not `Window::hide()`: on some Wayland compositors hiding the only window doesn't end
            // the loop, leaving a process with no UI.
            if let Err(e) = slint::quit_event_loop() {
                log::warn!("close-window: quit_event_loop: {}", describe(&e));
            }
        });
    }

    {
        let weak = app.as_weak();
        let state = state.clone();
        chrome.on_toggle_always_on_top(move || {
            let Some(ui) = weak.upgrade() else { return };
            let chrome = ui.global::<melodia_ui::WindowChrome>();
            let new = !chrome.get_always_on_top_active();
            let is_native = matches!(state.always_on_top.method, AlwaysOnTopMethod::Native);

            // Optimistic, so the icon flips on the click's own frame; reverted below on an error.
            chrome.set_always_on_top_active(new);

            // winit is UI-thread-only, so the native level is set here; the Linux backends go
            // through the task.
            if is_native {
                let level = if new { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal };
                let _ = ui.window().with_winit_window(|w| w.set_window_level(level));
            }

            let weak = weak.clone();
            let state_inner = state.clone();
            state.runtime.spawn(async move {
                if let Err(e) =
                    melodia_app::library::window::set_always_on_top(&state_inner, new).await
                {
                    log::warn!("set_always_on_top: {}", describe(&e));
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<melodia_ui::WindowChrome>().set_always_on_top_active(!new);
                        if is_native {
                            let revert =
                                if new { WindowLevel::Normal } else { WindowLevel::AlwaysOnTop };
                            let _ = ui.window().with_winit_window(|w| w.set_window_level(revert));
                        }
                    });
                }
            });
        });
    }

    chrome.on_restart_app(restart_toggle(
        app,
        state,
        "use_native_titlebar",
        melodia_app::library::window::set_use_native_titlebar,
    ));
    chrome.on_restart_tray(restart_toggle(
        app,
        state,
        "tray_enabled",
        melodia_app::library::window::set_tray_enabled,
    ));
    chrome.on_restart_backdrop(restart_toggle(
        app,
        state,
        "aurora_backdrop",
        melodia_app::library::window::set_aurora_backdrop,
    ));
}

/// Persists the value a restart dialog carries, then asks for the respawn. `Dialog.target-id`
/// holds the value and has to be read before the dispatcher wipes the routing payload. `setting`
/// names the write for the log, `persist` being a bare fn pointer.
fn restart_toggle(
    app: &AppWindow,
    state: &AppState,
    setting: &'static str,
    persist: fn(&Paths, bool) -> Result<(), AppError>,
) -> impl Fn() + 'static {
    let weak = app.as_weak();
    let paths = Arc::clone(&state.paths);
    move || {
        let Some(ui) = weak.upgrade() else { return };
        let on = ui.global::<melodia_ui::Dialog>().get_target_id() == 1;

        if let Err(e) = persist(&paths, on) {
            log::warn!("persist {setting} failed: {}", describe(&e));
            return;
        }

        super::request_respawn_and_quit();
    }
}
