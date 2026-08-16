//! Wires the `WindowChrome` global and the `Theme.use-native-titlebar`
//! property to the persisted setting, the `slint::Window` API, and
//! winit (via Slint's `unstable-winit-030` accessor).
//!
//! Six reasons it exists: hydrating `Theme.use-native-titlebar` (Slint reads
//! `Window.no-frame` once at first show, so it has to land in the gap between
//! `AppWindow::new()` and `app.run()` — exactly where `main.rs` calls [`install`]), the
//! window control callbacks ([`controls`]), window dragging ([`winit_filter`]), file-drop
//! coalescing ([`drop_coalescer`]), geometry ([`geometry`]) and the restart flow.
//!
//! **Dragging belongs at the winit layer.** `drag_window()` from a `TouchArea`'s
//! `pointer-event` leaks the grab: the compositor takes pointer ownership for the move and
//! the matching `Released` never reaches Slint, so a `TouchArea` that returned `GrabMouse`
//! stays pressed forever and every later click routes back to it. Slint's own resize-border
//! handler dodges this by intercepting `MouseInput { Pressed, Left }` before dispatch, and
//! this mirrors it against an atomic a Slint callback keeps in step with drag-area hover.
//!
//! **`no-frame` is sticky after first show**, so toggling the native titlebar needs a
//! fresh process: persist, then hand off to [`request_respawn_and_quit`], which arms
//! [`RESPAWN_AFTER_EXIT`] and quits the loop so `main()` falls through to shutdown before
//! the new process takes over. One function rather than three, since it owns the refusal.

mod controls;
mod drop_coalescer;
pub mod geometry;
mod winit_filter;

pub use drop_coalescer::{
    is_playlist_detail_open, is_queue_sheet_open, set_current_playlist_id, set_queue_sheet_open,
};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit::window::WindowLevel;

use crate::error::AppError;
use crate::services::always_on_top::AlwaysOnTopMethod;
use crate::services::toast::{self, ToastKind};
use crate::state::AppState;
use crate::{AppWindow, Theme};

/// Armed once the new setting is persisted, read at the very end of `main()` — after the
/// event loop has exited and every background task wound down. Spawning *before* shutdown
/// leaves two windows on screen while the old process tears down its runtime.
static RESPAWN_AFTER_EXIT: AtomicBool = AtomicBool::new(false);

/// Read by `main()` after `app.run()` returns.
pub fn should_respawn_after_exit() -> bool {
    RESPAWN_AFTER_EXIT.load(Ordering::SeqCst)
}

/// Explicit binary path for the post-exit respawn, set by the auto-updater at
/// install-success time while the path still resolves to the live binary.
/// [`respawn_target`] prefers it over asking the OS because an **atomic-swap** install
/// *renamed* the running binary to `<target>.old` — a move, not an unlink, so
/// `/proc/self/exe` reports that stale path with a straight face and respawning from it
/// relaunches the *old* binary, which nothing can recover after the fact. The titlebar and
/// tray restarts never set this: no install happened, so the OS answer is right.
static RESPAWN_EXE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Record the binary path to relaunch on exit. The auto-updater calls this on a
/// successful install with `install_target()` captured *before* the swap.
pub fn set_respawn_exe(path: PathBuf) {
    match RESPAWN_EXE.lock() {
        Ok(mut slot) => *slot = Some(path),
        Err(_) => log::warn!(
            "updater: RESPAWN_EXE lock poisoned; \
             restart may relaunch the wrong binary"
        ),
    }
}

/// The explicit respawn path, if the updater recorded one this session. Private, so no
/// caller reaches the slot without the fallback that makes an empty one mean "ask the OS".
fn respawn_exe() -> Option<PathBuf> {
    RESPAWN_EXE.lock().ok().and_then(|slot| slot.clone())
}

/// The binary the post-exit respawn should launch: the path the updater recorded before
/// its install swapped the file, else the running binary's own. `None` only when the OS
/// refused to say where we are running from, so there is nothing to come back to.
pub fn respawn_target() -> Option<PathBuf> {
    if let Some(recorded) = respawn_exe() {
        return Some(recorded);
    }
    match crate::services::current_exe() {
        Ok(exe) => Some(exe),
        Err(e) => {
            log::warn!("respawn: executable lookup failed: {e}");
            None
        }
    }
}

/// Arm the post-exit respawn and quit the event loop — unless there is no binary to come
/// back to, in which case leave the app running and say so.
///
/// All three restart paths go through here because getting the check wrong costs the user
/// their session: past `quit_event_loop()` the window is gone, and a failed `exec` in
/// `crate::shutdown::respawn_if_requested` has nothing to fall back to. Hence resolving
/// the target *before* the exit. Every caller has persisted its setting by now, so a
/// refusal still applies on the next manual launch, which is what the toast says.
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
        // Nothing reads the flag while the loop runs, but an ordinary window close later
        // in the session would, and relaunching out of that is not what was asked for.
        RESPAWN_AFTER_EXIT.store(false, Ordering::SeqCst);
        log::warn!("restart: quit_event_loop: {e}");
    }
}

/// Hydrate `Theme.use-native-titlebar` from the persisted setting and wire the
/// `WindowChrome` callbacks. Must run between `AppWindow::new()` and `app.run()`.
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

/// Push the cached capability and persisted pinned state into `WindowChrome`, and
/// re-apply at the OS level if the window was pinned last time. The Linux re-apply
/// waits, so `KWin` / GNOME have registered the window — `MakeAbove` against a
/// not-yet-shown one quietly fails on bare `window-calls`.
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
            // `Handle::clone()` releases the borrow on `state.runtime` so the same
            // `state` can be moved into the async block.
            state.runtime.clone().spawn(async move {
                // KWin enumerates `workspace.stackingOrder` by PID, so the window has to
                // be mapped by the compositor first.
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
