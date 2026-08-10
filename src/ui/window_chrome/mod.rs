//! Wires the `WindowChrome` global and the `Theme.use-native-titlebar`
//! property to the persisted setting, the `slint::Window` API, and
//! winit (via Slint's `unstable-winit-030` accessor).
//!
//! ## Why this module exists
//!
//! 1. **Hydrating `Theme.use-native-titlebar` *before* `app.run()`.** Slint
//!    reads `Window.no-frame` once when the OS window first shows, so the
//!    persisted value has to land in the gap between `AppWindow::new()` and
//!    `app.run()`. [`install`] is called from `main.rs` in exactly that gap.
//! 2. **Window control callbacks** ([`controls`]) — `minimize` /
//!    `toggle-maximize` / `close-window` / `toggle-always-on-top` map cleanly
//!    to winit / D-Bus operations.
//! 3. **Window dragging at the winit layer, not Slint** ([`winit_filter`]).
//!    Calling `drag_window()` from a `TouchArea`'s `pointer-event` leaks the
//!    grab: the compositor takes pointer ownership for the move and the
//!    matching `Released` never reaches Slint, so the `TouchArea` that
//!    returned `GrabMouse` stays pressed forever and every later click routes
//!    back to it — killing interactivity across the window. Slint's own
//!    resize-border handler dodges this by intercepting
//!    `MouseInput { Pressed, Left }` in `i-slint-backend-winit::event_loop`
//!    before dispatch, and we mirror it: a Slint callback tracks drag-area
//!    hover in an atomic, and `on_winit_window_event` answers Press with
//!    `EventResult::PreventDefault` while it's set.
//! 4. **OS file-drop coalescing** ([`drop_coalescer`]) — winit fires
//!    `DroppedFile` once per file; multi-file drops become one
//!    `queue_import_files` call.
//! 5. **Window geometry** ([`geometry`]) — save/restore across hide-to-tray
//!    and the maximize seed.
//! 6. **Restart flow** ([`controls::wire`]'s `on_restart_app`). `no-frame` is
//!    sticky after first show, so toggling the native titlebar needs a fresh
//!    process: persist through `library::window::set_use_native_titlebar`,
//!    then hand off to [`request_respawn_and_quit`], which sets
//!    [`RESPAWN_AFTER_EXIT`] and quits the event loop so `main()` falls through
//!    to `save_state_on_exit` and shuts the runtime down before the new process
//!    takes over. It is one function rather than three copies because it also
//!    owns the refusal: with no binary left to relaunch it keeps the app up and
//!    toasts instead, the setting having already been persisted. The tray toggle
//!    and the updater's "Restart Now" take the same call; the updater
//!    additionally records the binary path via [`set_respawn_exe`], captured
//!    before the install swapped the binary on disk.

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
use crate::services::toast::{self, ToastKind};
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

/// Explicit binary path for the post-exit respawn. Set by the
/// auto-updater at install-success time, while the path still resolves
/// to the live binary. [`respawn_target`] prefers it over asking the OS,
/// because by the time the respawn runs the updater has already replaced
/// the binary on disk and an **atomic-swap** install *renamed* the
/// running one to `<target>.old` — a move, not an unlink, so
/// `/proc/self/exe` reports that stale path with a straight face and
/// respawning from it relaunches the *old* binary. That is what keeps
/// this static necessary: the RPM/DEB half of the problem, a `(deleted)`
/// marker on an unlinked path, is now resolved centrally by
/// [`crate::services::current_exe`], but nothing can recover a rename
/// after the fact. The titlebar and tray restarts never set this — no
/// install happened, so the OS answer is the right one.
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
/// `None` for a plain titlebar-mode restart. Private: [`respawn_target`] is
/// what the rest of the crate asks, so no caller can read the slot without
/// the fallback that makes an empty one mean "ask the OS".
fn respawn_exe() -> Option<PathBuf> {
    RESPAWN_EXE.lock().ok().and_then(|slot| slot.clone())
}

/// The binary the post-exit respawn should launch: the path the updater
/// recorded before its install swapped the file, else the running
/// binary's own. `None` only when there is neither — the OS refused to
/// say where we are running from, so there is nothing to come back to.
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

/// Arm the post-exit respawn and quit the event loop — unless there is no
/// binary to come back to, in which case leave the app running and say so.
///
/// All three restart paths go through here (the titlebar and tray toggles
/// and the updater's "Restart Now") because the check is the same one and
/// getting it wrong costs the user their session: past `quit_event_loop()`
/// the window is gone, and a failed `exec` in
/// `crate::shutdown::respawn_if_requested` has nothing left to fall back
/// to — the app simply vanishes. So the target is resolved *before* the
/// exit rather than after it. Every caller has already persisted its
/// setting by this point, so a refusal still applies on the next manual
/// launch, which is what the toast tells the user.
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
        // The loop is still running, so nothing will read the flag now —
        // but an ordinary window close later in the session would, and
        // relaunching out of that is not what the user asked for.
        RESPAWN_AFTER_EXIT.store(false, Ordering::SeqCst);
        log::warn!("restart: quit_event_loop: {e}");
    }
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
