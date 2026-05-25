//! Wires the `WindowChrome` global and the `Theme.use-native-titlebar`
//! property to the persisted setting, the `slint::Window` API, and
//! winit (via Slint's `unstable-winit-030` accessor).
//!
//! ## Why this module exists
//!
//! Five concerns live here:
//!
//! 1. **Hydrating `Theme.use-native-titlebar` *before* `app.run()`.** Slint
//!    reads `Window.no-frame` once when the OS window first shows, so the
//!    persisted value must reach the global in the gap between
//!    `AppWindow::new()` (component constructed, OS window not yet shown)
//!    and `app.run()` (event loop starts → first show happens). [`install`]
//!    is called from `main.rs` in that exact gap.
//! 2. **Window control callbacks** ([`controls`]). `minimize` /
//!    `toggle-maximize` / `close-window` / `toggle-always-on-top` map
//!    cleanly to winit / D-Bus operations.
//! 3. **Window dragging at the winit layer (not Slint)** ([`winit_filter`]).
//!    Calling winit's `drag_window()` from inside a Slint `TouchArea`'s
//!    `pointer-event` leaks the grab: the OS / compositor takes pointer
//!    ownership for the move (Wayland `xdg_toplevel.move`, X11 cursor
//!    un-grab, macOS dropped release), and the matching `Released` event
//!    never reaches Slint. The `TouchArea` returned `GrabMouse` on
//!    Pressed and stays "pressed" indefinitely — every subsequent click
//!    routes back to the orphaned `TouchArea` and triggers another
//!    drag, killing all interactivity in the rest of the window.
//!
//!    Slint's own resize-border handler dodges this by intercepting
//!    `MouseInput { Pressed, Left }` at the winit layer in
//!    `i-slint-backend-winit::event_loop` and returning before the
//!    event is dispatched to any Slint item. We mirror that pattern:
//!    track hover state of the titlebar drag area via a Slint callback
//!    into an atomic, then in `on_winit_window_event` intercept Press
//!    when hover is true and return `EventResult::PreventDefault`.
//!    Slint never sees the press, so no `TouchArea` grab is engaged.
//! 4. **OS file-drop coalescing** ([`drop_coalescer`]). Winit fires
//!    `WindowEvent::DroppedFile` once per file; multi-file drops are
//!    batched into a single `queue_import_files` call.
//! 5. **Restart flow** ([`controls::wire`]'s `on_restart_app`).
//!    `Window.no-frame` is sticky after first show, so toggling the
//!    native titlebar requires a fresh process. `restart-app` persists
//!    the new value through `library::window::set_use_native_titlebar`,
//!    sets [`RESPAWN_AFTER_EXIT`], then calls `slint::quit_event_loop()`
//!    so `main()` falls through to `save_state_on_exit` and the runtime
//!    shuts down cleanly before the new process takes over. The
//!    auto-updater's "Restart Now" reuses the same flag but additionally
//!    records the binary path via [`set_respawn_exe`] (captured before
//!    the install swapped the binary on disk).

mod controls;
mod drop_coalescer;
pub mod geometry;
mod winit_filter;

pub use drop_coalescer::{
    is_playlist_detail_open, is_queue_sheet_open, set_current_playlist_id,
    set_queue_sheet_open,
};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::WindowLevel;

use crate::error::AppError;
use crate::services::always_on_top::AlwaysOnTopMethod;
use crate::state::AppState;
use crate::{AppWindow, Theme};

/// Set to `true` by the restart-confirm dialog after the new
/// `use_native_titlebar` value has been persisted; read once at the
/// very end of `main()` (after the event loop has exited and every
/// background task has wound down) to decide whether to respawn the
/// binary. Spawning *before* shutdown leaves two windows on screen
/// while the old process is still tearing down its tokio runtime,
/// which the user observes as "old window stays, new one opens, old
/// becomes unclickable but doesn't disappear". Deferring keeps the
/// transition serial: old window closes → process exits → new
/// process starts.
static RESPAWN_AFTER_EXIT: AtomicBool = AtomicBool::new(false);

/// Read by `main()` after `app.run()` returns and the runtime has
/// shut down. Resetting it would race with `main()`'s read, but that
/// would only mean a missed restart on a logically impossible second
/// invocation — there's only one event loop per process, and the
/// flag is set during its final tick.
pub fn should_respawn_after_exit() -> bool {
    RESPAWN_AFTER_EXIT.load(Ordering::SeqCst)
}

/// Set the respawn flag. Used by the auto-updater's "Restart Now"
/// flow — calling this followed by `slint::quit_event_loop()` exits
/// the event loop and re-launches the binary via
/// `crate::shutdown::respawn_if_requested()` (the last step of
/// `main()`). The static stays private; callers go through this
/// accessor so the dataflow is greppable.
pub fn request_respawn() {
    RESPAWN_AFTER_EXIT.store(true, Ordering::SeqCst);
}

/// Explicit binary path for the post-exit respawn. Set by the
/// auto-updater at install-success time, while `current_exe()` still
/// resolves to the live binary. `shutdown::respawn_if_requested`
/// prefers this over `current_exe()` because, by the time it runs, the
/// updater has already replaced the binary on disk: `current_exe()`
/// would resolve to the stale `<target>.old` (atomic-swap install) or
/// a `<path> (deleted)` path (RPM/DEB install), so respawning from it
/// would relaunch the *old* binary or fail outright. The titlebar
/// restart never sets this — no install happened, so its
/// `current_exe()` fallback stays correct.
static RESPAWN_EXE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Record the binary path to relaunch on exit. The auto-updater calls
/// this on a successful install with `services::updater::install_target()`
/// captured *before* the swap, while the path is still live.
pub fn set_respawn_exe(path: PathBuf) {
    match RESPAWN_EXE.lock() {
        Ok(mut slot) => *slot = Some(path),
        Err(_) => log::warn!(
            "updater: RESPAWN_EXE lock poisoned; \
             restart may relaunch the wrong binary"
        ),
    }
}

/// The explicit respawn path, if the updater recorded one this session.
/// `None` for a plain titlebar-mode restart.
pub fn respawn_exe() -> Option<PathBuf> {
    RESPAWN_EXE.lock().ok().and_then(|slot| slot.clone())
}

/// Hydrate `Theme.use-native-titlebar` from the persisted setting and
/// wire the `WindowChrome` callbacks. Must be called between
/// `AppWindow::new()` and `app.run()` — see module docs.
pub fn install(app: &AppWindow, state: &AppState) -> Result<(), AppError> {
    let settings = crate::library::settings::get_settings(state)?;
    let use_native = settings.window.use_native_titlebar;

    app.global::<Theme>().set_use_native_titlebar(use_native);

    let drag_hover = Arc::new(AtomicBool::new(false));

    winit_filter::install(app, state, drag_hover.clone());
    controls::wire(app, state, drag_hover);
    seed_always_on_top(app, state, settings.window.always_on_top);

    Ok(())
}

/// Push the cached capability + persisted pinned state into `WindowChrome`,
/// and re-apply at the OS level if the user had the window pinned last
/// time. The Linux re-apply runs after a short delay so `KWin` / GNOME have
/// already registered the window — calling `MakeAbove` against a not-yet-
/// shown window quietly fails on bare `window-calls`.
fn seed_always_on_top(app: &AppWindow, state: &AppState, persisted_pinned: bool) {
    let chrome = app.global::<crate::WindowChrome>();
    chrome.set_always_on_top_supported(state.always_on_top.supported);
    chrome.set_always_on_top_active(persisted_pinned);

    if !state.always_on_top.supported || !persisted_pinned {
        return;
    }

    match state.always_on_top.method {
        AlwaysOnTopMethod::Native => {
            let _ = app.window().with_winit_window(|w| {
                w.set_window_level(WindowLevel::AlwaysOnTop);
            });
        }
        #[cfg(target_os = "linux")]
        AlwaysOnTopMethod::KwinDbus | AlwaysOnTopMethod::GnomeExtension => {
            let state = state.clone();
            // `Handle::clone()` releases the borrow on `state.runtime`
            // so the same `state` can be moved into the async block.
            state.runtime.clone().spawn(async move {
                // KWin enumerates `workspace.stackingOrder` by PID — the
                // window has to be mapped by the compositor first. 300 ms
                // is comfortably past first paint on every system I've
                // tested and still feels instantaneous to the user.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if let Err(e) = crate::services::always_on_top::apply(&state, true).await {
                    log::warn!("startup always_on_top re-apply: {e}");
                }
            });
        }
        AlwaysOnTopMethod::Unsupported => {}
    }
}

#[cfg(test)]
#[path = "tests/respawn_tests.rs"]
mod tests;
