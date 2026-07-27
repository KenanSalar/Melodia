//! Wire the `WindowChrome` callback global to OS-level window-control
//! operations and the restart flow.
//!
//! Each callback runs on the Slint UI thread (callback dispatch
//! guarantees that), so winit calls are safe directly without an
//! event-loop hop. Persistence work (`set_always_on_top`,
//! `set_use_native_titlebar`) is dispatched to the tokio runtime so
//! the UI thread isn't blocked.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::WindowLevel;

use crate::services::always_on_top::AlwaysOnTopMethod;
use crate::state::AppState;
use crate::AppWindow;

pub(super) fn wire(app: &AppWindow, state: &AppState, drag_hover: Arc<AtomicBool>) {
    let chrome = app.global::<crate::WindowChrome>();

    {
        let weak = app.as_weak();
        chrome.on_minimize(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Call winit directly instead of `slint::Window::set_minimized`.
            // Slint's adapter tracks an internal `is_minimized` cache that
            // it tries to keep in sync via `WindowEvent::Occluded` /
            // configure events, but on Wayland `winit::Window::is_minimized`
            // returns `None` (the protocol doesn't expose minimize state
            // through `xdg_toplevel`). After the user restores the window
            // from the taskbar, Slint's cache stays stuck at `true`, so a
            // second `set_minimized(true)` looks like "no change" and never
            // reaches the OS — the user's symptom is "minimize works once,
            // then never again". Driving winit directly sidesteps the
            // cache: `set_minimized(true)` always issues a fresh
            // `xdg_toplevel.set_minimized` to the compositor.
            let _ = ui.window().with_winit_window(|w| w.set_minimized(true));
            // Keep the tray-bridge visibility shadow in sync. The tray
            // "Show / Hide" toggle reads it; without this drop, the next
            // tray click sees the window as still-visible and unmaps the
            // (already-minimized) surface — the user then has to click the
            // tray a second time to actually restore the window. The
            // visualizer's Timer gates on it too, so this is also what
            // stops it drawing for a window nobody can see.
            crate::ui::tray_bridge::set_window_visible(&ui, false);
        });
    }

    {
        let weak = app.as_weak();
        chrome.on_toggle_maximize(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Read **and** write via winit directly, mirroring the
            // minimize handler. The write would otherwise go through
            // `slint::Window::set_maximized`, which only forwards to
            // winit when its cached `maximized` flag differs from the
            // new value (`winitwindowadapter.rs:1328`). That cache is
            // refreshed on configure events, so any state change that
            // hasn't yet round-tripped through the compositor — Win+↑,
            // snap-to-edge, the WM's own maximize gesture — leaves a
            // window where winit *is* maximized but Slint's cache
            // still says false (or vice versa). When that happens, the
            // toggle becomes a no-op and the user has to click twice.
            //
            // Cross-platform note: `winit::Window::is_maximized()`
            // returns `bool` (not `Option<bool>`) on Wayland, X11,
            // Win32 and macOS, so this read is safe everywhere — the
            // `is_minimized` foot-gun (Wayland returns `None`) does
            // not apply here.
            let _ = ui.window().with_winit_window(|w| {
                w.set_maximized(!w.is_maximized());
            });
        });
    }

    {
        let weak = app.as_weak();
        chrome.on_close_window(move || {
            // Close-to-tray: when the user enabled the setting AND a tray
            // icon is actually active, hide the window instead of quitting.
            // `should_hide_to_tray` returns false when no tray exists, so
            // this can never strand the user with a hidden window. The hide
            // is deferred via `invoke_from_event_loop` — calling it inline
            // from this callback (which runs inside a winit `WindowEvent`
            // dispatch) trips Slint's "references to the window still exist"
            // warning, because the dispatcher still holds the window `Arc`.
            if crate::ui::tray_bridge::should_hide_to_tray() {
                let weak = weak.clone();
                if let Err(e) = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        crate::ui::tray_bridge::hide_window(&ui);
                    }
                }) {
                    log::warn!("close-to-tray: schedule hide: {e}");
                }
                return;
            }
            // `Window::hide()` is platform-dependent — on some Wayland
            // compositors hiding the only visible window doesn't
            // reliably end the event loop, leaving the process alive
            // with no UI. `quit_event_loop()` is explicit: it
            // schedules `app.run()` to return, after which `main()`
            // runs `save_state_on_exit` and tears the runtime down.
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

            // Optimistic flip so the icon + tooltip update on the same
            // frame as the click. Reverted below if the backend errors.
            chrome.set_always_on_top_active(new);

            // Native path is winit-only and must run on the UI thread,
            // so do it before spawning the persistence task. Linux paths
            // go through `library::window::set_always_on_top` which
            // routes to the `KWin` / GNOME backend (each is wrapped in
            // `spawn_blocking` internally).
            if is_native {
                let level = if new { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal };
                let _ = ui.window().with_winit_window(|w| w.set_window_level(level));
            }

            let weak = weak.clone();
            let state_inner = state.clone();
            state.runtime.spawn(async move {
                if let Err(e) = crate::library::window::set_always_on_top(&state_inner, new).await {
                    log::warn!("set_always_on_top: {e}");
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.global::<crate::WindowChrome>().set_always_on_top_active(!new);
                        // Native applied via winit on the UI thread above —
                        // roll the OS-level state back too so it stays in
                        // sync with the Slint property we just reverted.
                        if is_native {
                            let revert = if new { WindowLevel::Normal } else { WindowLevel::AlwaysOnTop };
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

    {
        let weak = app.as_weak();
        let state = state.clone();
        chrome.on_restart_app(move || {
            let Some(ui) = weak.upgrade() else { return };
            // `Dialog.target-id` carries the requested new value (1 =
            // native titlebar, 0 = custom). Read it before the
            // dispatcher wipes the routing payload (see the default
            // `accepted` handler in `globals.slint`).
            let target = ui.global::<crate::Dialog>().get_target_id();
            let on = target == 1;

            if let Err(e) = crate::library::window::set_use_native_titlebar(&state, on) {
                log::warn!("persist use_native_titlebar failed: {e}");
                return;
            }

            // Defer the actual respawn to `main()`'s exit path —
            // spawning here while the old event loop is still wrapping
            // up leaves the user staring at two windows, the old one
            // unresponsive but still visible until the old process
            // finishes its `tracker.wait()`. With the flag, the old
            // process tears down completely first, then forks the
            // new one as the very last step before returning from
            // `main()`.
            super::RESPAWN_AFTER_EXIT.store(true, Ordering::SeqCst);
            if let Err(e) = slint::quit_event_loop() {
                log::warn!("quit_event_loop on restart: {e}");
            }
        });
    }

    {
        let weak = app.as_weak();
        let state = state.clone();
        chrome.on_restart_tray(move || {
            let Some(ui) = weak.upgrade() else { return };
            // `Dialog.target-id` carries the requested new value (1 = tray
            // enabled, 0 = disabled). Read it before the dispatcher wipes
            // the routing payload.
            let target = ui.global::<crate::Dialog>().get_target_id();
            let on = target == 1;

            if let Err(e) = crate::library::window::set_tray_enabled(&state, on) {
                log::warn!("persist tray_enabled failed: {e}");
                return;
            }

            // Same deferred-respawn rationale as `on_restart_app`: tearing
            // the old process down fully before forking the new one.
            super::RESPAWN_AFTER_EXIT.store(true, Ordering::SeqCst);
            if let Err(e) = slint::quit_event_loop() {
                log::warn!("quit_event_loop on tray restart: {e}");
            }
        });
    }
}
