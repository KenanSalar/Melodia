//! The window itself, through winit where Slint's window API falls short: controls, dragging and
//! resizing, geometry, file drops and the restart flow.
//!
//! **Dragging and resizing belong at the winit layer.** `drag_window()` from a `pointer-event`
//! leaks the grab: the compositor takes the pointer for the move, the matching `Released` never
//! reaches Slint, and the `TouchArea` stays pressed, taking every later click. So the filter
//! intercepts `MouseInput { Pressed, Left }` before dispatch, against cells kept in step with
//! drag-region and resize-grab hover.
//!
//! **The native titlebar toggle restarts**, not for the frame, which Slint applies live through
//! `no-frame`, but because boot is the only writer of `Theme.use-native-titlebar` and of
//! `Settings.match-unfocused-bg`, which a KDE enable turns on alongside.

mod controls;
mod drag_region;
mod file_drop;
pub mod geometry;
#[cfg(target_os = "windows")]
pub mod parked_loop;
pub mod pointer;
mod resize_grab;
mod resize_release;
mod winit_filter;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::WindowLevel;

use melodia_app::state::AppState;
use melodia_core::error::{AppError, describe};
use melodia_core::utils::toast::{self, ToastKind};
#[cfg(target_os = "linux")]
use melodia_platform::services::platform::always_on_top;
use melodia_platform::services::platform::always_on_top::AlwaysOnTopMethod;
use melodia_ui::{AppWindow, Theme};

/// Read at the end of `main()`, past shutdown: spawning before it leaves two windows on screen
/// while the old process tears down.
static RESPAWN_AFTER_EXIT: AtomicBool = AtomicBool::new(false);

pub fn should_respawn_after_exit() -> bool {
    RESPAWN_AFTER_EXIT.load(Ordering::SeqCst)
}

/// The binary the updater installed. An atomic-swap install renames the running binary to
/// `<target>.old` and `/proc/self/exe` follows the rename, so the OS's answer would relaunch the
/// old one.
static RESPAWN_EXE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Called by the updater on a successful install, with `install_target()` captured before the swap.
pub fn set_respawn_exe(path: PathBuf) {
    match RESPAWN_EXE.lock() {
        Ok(mut slot) => *slot = Some(path),
        Err(_) => log::warn!(
            "updater: RESPAWN_EXE lock poisoned; \
             restart may relaunch the wrong binary"
        ),
    }
}

fn respawn_exe() -> Option<PathBuf> {
    RESPAWN_EXE.lock().ok().and_then(|slot| slot.clone())
}

/// The binary the post-exit respawn launches: the updater's recorded path, else the running one.
/// `None` only when the OS won't say where we run from.
pub fn respawn_target() -> Option<PathBuf> {
    if let Some(recorded) = respawn_exe() {
        return Some(recorded);
    }
    match melodia_core::utils::exe::current_exe() {
        Ok(exe) => Some(exe),
        Err(e) => {
            log::warn!("respawn: executable lookup failed: {}", describe(&e));
            None
        }
    }
}

/// Arms the post-exit respawn and quits the loop, unless there is no binary to come back to, in
/// which case the app stays up and says so.
///
/// The check sits on this side of the exit: past `quit_event_loop()` the window is gone, and a
/// failed `exec` in `shutdown::respawn_if_requested` has nothing to fall back to. Every caller has
/// persisted its setting by then, so the toast's "next launch" is honest.
pub fn request_respawn_and_quit() {
    match respawn_target() {
        Some(exe) if exe.exists() => {}
        gone => {
            let reason = gone.map_or_else(
                || "the executable path is unavailable".to_owned(),
                |exe| format!("{} no longer exists", exe.display()),
            );
            log::warn!("restart: staying up — {reason}");
            toast::notify(ToastKind::RestartRequired, "");
            return;
        }
    }

    RESPAWN_AFTER_EXIT.store(true, Ordering::SeqCst);
    if let Err(e) = slint::quit_event_loop() {
        // Or an ordinary close later in the session reads it and relaunches.
        RESPAWN_AFTER_EXIT.store(false, Ordering::SeqCst);
        log::warn!("restart: quit_event_loop: {}", describe(&e));
    }
}

/// The shown window's Win32 `HWND`, `None` before the first show. Fetched per call, the accessor
/// only borrowing for the closure.
///
/// The one hop for its two consumers, souvlaki's SMTC attach in `boot::ui_setup::install` and
/// `dwm_titlebar` through `ui::appearance::theme_paint`.
#[cfg(target_os = "windows")]
pub fn win32_hwnd(app: &AppWindow) -> Option<*mut std::ffi::c_void> {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    app.window()
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
            _ => None,
        })
        .flatten()
}

/// Hydrates `Theme.use-native-titlebar` and wires the `WindowChrome` callbacks. Runs between
/// `AppWindow::new()` and `app.show()`, so the window maps with its frame decided.
pub fn install(app: &AppWindow, state: &AppState) -> Result<(), AppError> {
    let settings = melodia_app::library::settings::get_settings(&state.paths)?;
    let use_native = settings.window.use_native_titlebar;

    app.global::<Theme>().set_use_native_titlebar(use_native);

    let press_targets = winit_filter::PressTargets {
        drag_region: drag_region::wire(app),
        resize: resize_grab::wire(app),
    };

    geometry::wire_frame_changes(app);
    winit_filter::install(app, state, press_targets);
    controls::wire(app, state);
    seed_always_on_top(app, state, settings.window.always_on_top);

    Ok(())
}

/// Seeds the pin's capability and persisted state, re-applies a persisted pin once the window can
/// take one, and on Linux follows a pin moved outside Melodia.
fn seed_always_on_top(app: &AppWindow, state: &AppState, persisted_pinned: bool) {
    let chrome = app.global::<melodia_ui::WindowChrome>();
    chrome.set_always_on_top_supported(state.always_on_top.supported);
    chrome.set_always_on_top_active(persisted_pinned);

    if !state.always_on_top.supported {
        return;
    }

    match state.always_on_top.method {
        AlwaysOnTopMethod::Native => {
            if !persisted_pinned {
                return;
            }
            let weak = app.as_weak();
            if let Err(e) = slint::spawn_local(async move {
                let Some(ui) = weak.upgrade() else { return };
                match ui.window().winit_window().await {
                    Ok(window) => window.set_window_level(WindowLevel::AlwaysOnTop),
                    Err(e) => log::warn!("startup always_on_top re-apply: {}", describe(&e)),
                }
            }) {
                log::warn!("startup always_on_top re-apply: schedule: {}", describe(&e));
            }
        }
        #[cfg(target_os = "linux")]
        AlwaysOnTopMethod::KwinDbus | AlwaysOnTopMethod::GnomeExtension => {
            let (reports, reported) = tokio::sync::watch::channel(persisted_pinned);
            follow_reported_pin(app, state, reported);

            let state = state.clone();
            state.runtime.clone().spawn(async move {
                let method = state.always_on_top.method;
                let data_dir = &state.paths.data_dir;
                if persisted_pinned {
                    // The compositor has to have mapped the window: `KWin` finds it by PID in
                    // `workspace.stackingOrder`, and bare `window-calls` quietly fails before then.
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    if let Err(e) = always_on_top::apply(method, data_dir, true).await {
                        log::warn!("startup always_on_top re-apply: {}", describe(&e));
                    }
                }
                if let Err(e) = always_on_top::watch_changes(method, data_dir, reports).await {
                    log::warn!("always_on_top watch: {}", describe(&e));
                }
            });
        }
        AlwaysOnTopMethod::Unsupported => {}
    }
}

/// Mirror each pin the window manager reports into `WindowChrome` and the persisted setting, so
/// `KWin`'s keep-above titlebar button moves Melodia's pin button too. A report matching the
/// button is a toggle of Melodia's own coming back, and changes nothing. GNOME and winit report
/// nothing, so there a pin moved from the titlebar leaves the button behind.
#[cfg(target_os = "linux")]
fn follow_reported_pin(
    app: &AppWindow,
    state: &AppState,
    mut reported: tokio::sync::watch::Receiver<bool>,
) {
    let weak = app.as_weak();
    let state = state.clone();
    // One loop, so a burst of reports persists in the order the window manager sent them.
    let followed = slint::spawn_local(async_compat::Compat::new(async move {
        while reported.changed().await.is_ok() {
            let pinned = *reported.borrow();
            {
                let Some(ui) = weak.upgrade() else { return };
                let chrome = ui.global::<melodia_ui::WindowChrome>();
                if chrome.get_always_on_top_active() == pinned {
                    continue;
                }
                chrome.set_always_on_top_active(pinned);
            }
            if let Err(e) =
                melodia_app::library::window::record_always_on_top(&state.paths, pinned).await
            {
                log::warn!("record always_on_top: {}", describe(&e));
            }
        }
    }));
    if let Err(e) = followed {
        log::warn!("always_on_top follower: spawn_local: {}", describe(&e));
    }
}

#[cfg(test)]
#[path = "tests/always_on_top_tests.rs"]
mod always_on_top_tests;
#[cfg(test)]
#[path = "tests/respawn_tests.rs"]
mod tests;
