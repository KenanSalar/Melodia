//! Glue between the system tray ([`crate::services::tray`]) and the rest of the app.
//!
//! Three pieces:
//!
//! 1. **Action receiver** — a tracked tokio task draining the tray's
//!    `mpsc::Receiver<TrayAction>`. Playback actions are fed into the same
//!    [`EventSink`] (`SlintEventSink`) the OS media controls use; window
//!    actions hop to the UI thread via `slint::invoke_from_event_loop`.
//! 2. **State subscriber** — watches `sinks.view_model` and pushes a
//!    [`TraySnapshot`] (tooltip + play/pause label) into the tray. On Linux
//!    this is a tokio task that owns the (`Send`) `LinuxTray`; on Windows /
//!    macOS the tray handle is `!Send`, so it is a `slint::spawn_local`
//!    future on the UI thread calling the `update_tray` free function.
//! 3. **Close-to-tray state** — process-global atomics read by
//!    `window_chrome` to decide whether a window-close hides to tray or
//!    quits, plus the window-visibility shadow the tray "Show / Hide" toggles
//!    against (`winit::Window::is_visible()` is `None` on Wayland).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use slint::ComponentHandle;

use crate::player::event_sink::{EventSink, PlayerEvent};
use crate::player::state::PlayerViewModelLight;
use crate::services::tray::{self, TRAY_ACTION_CHANNEL_CAP, TrayAction, TraySnapshot};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use crate::ui::shell::event_sink::SlintEventSink;
use crate::{AppWindow, Settings, Visualizer};

/// User preference: hide the window to tray on close instead of quitting.
/// Mirrors `settings.tray.close_to_tray`; seeded at startup and updated by
/// the Settings toggle so `window_chrome`'s close handlers read it without
/// touching disk.
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(false);

/// `true` when the main window is currently shown. Source of truth for the
/// tray "Show / Hide" toggle — `winit::Window::is_visible()` returns `None`
/// on Wayland, so the intended state is shadowed here.
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

/// `true` once a tray icon has actually been created. Close-to-tray falls
/// back to quitting when this is `false` — otherwise enabling the setting on
/// a session with no tray (vanilla GNOME) would strand the user with a
/// hidden window and no way to bring it back.
static TRAY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Window geometry (size + position) captured the instant the window was
/// hidden, re-applied on the next show. Hiding destroys the winit window;
/// showing recreates it, and Slint's first layout pass on the new window
/// would otherwise snap it to the content-preferred (≈ minimum) width and
/// let the compositor re-place it — the same reason
/// `window_chrome::geometry::restore` exists for the first-launch path.
static SAVED_WINDOW_GEOM: Mutex<Option<(slint::PhysicalSize, slint::PhysicalPosition)>> =
    Mutex::new(None);

/// How many times a tray-show re-asserts the saved geometry before giving up.
/// Each attempt is one 16 ms timer tick (see `reschedule_geometry_restore`).
const RESTORE_ATTEMPTS: u8 = 8;

/// Update the close-to-tray preference. Called by the Settings toggle and
/// once at startup to seed from `settings.json`.
pub fn set_close_to_tray(on: bool) {
    CLOSE_TO_TRAY.store(on, Ordering::Relaxed);
}

/// `true` when a window-close should hide to tray rather than quit: the user
/// enabled the setting *and* a tray icon is actually active.
pub fn should_hide_to_tray() -> bool {
    CLOSE_TO_TRAY.load(Ordering::Relaxed) && TRAY_ACTIVE.load(Ordering::Relaxed)
}

/// Update the visibility shadow from outside this module. Three call sites:
/// the titlebar minimize handler (drops it to `false` after
/// `xdg_toplevel.set_minimized`), the deferred `is_minimized` probe the winit
/// filter runs after a focus loss (catches a minimize the OS chrome or the
/// taskbar drove), and the winit `Focused(true)` listener (raises it back to
/// `true` when the window is restored). Without these hooks the shadow
/// desynchronises from reality and the next tray click does the wrong thing —
/// most visibly: after a button-minimize the tray click hides the
/// (already-minimized) surface, forcing the user to click the tray a second
/// time to actually restore the window.
///
/// `Visualizer.window-shown` moves with it. The visualizer's Timer gates on
/// the Slint half and its tick reads the atomic, so the two must never
/// disagree — hence one writer rather than two. That gate is also why hiding
/// has to hand the visualizer its own notice rather than leave it to the next
/// tick: lowering `window-shown` can stop the Timer that would have run it.
pub fn set_window_visible(ui: &AppWindow, visible: bool) {
    WINDOW_VISIBLE.store(visible, Ordering::Relaxed);
    let viz = ui.global::<Visualizer>();
    viz.set_window_shown(visible);
    if visible {
        // The strip may have gone dormant while it was off screen, leaving its
        // Timer down at the polling rate — where it would take another half a
        // second to work out that it is being drawn again. This *is* that
        // notice, so hand it over rather than making it infer the same thing.
        viz.set_dormant(false);
    } else {
        // The audio tap can't wait for the strip's next tick to be disarmed:
        // `window-shown` gates that Timer too, so on an already-settled drawing
        // the `set_window_shown` above just stopped it outright.
        viz.invoke_window_hidden();
    }
}

/// Read the visibility shadow. For UI work that keeps running off a `Timer`
/// while the window is hidden — Slint timers fire off the event loop, not the
/// render loop, and the loop deliberately stays alive through a close-to-tray
/// hide. Covers the tray hide and every minimize the OS will admit to; Wayland
/// tells a client nothing about being minimized, so this is a cheap gate rather
/// than a guarantee, and `ui::visualizer::pulse` is what covers the rest.
#[must_use]
pub fn is_window_visible() -> bool {
    WINDOW_VISIBLE.load(Ordering::Relaxed)
}

/// Hide the main window to the tray. Snapshots the current size + position
/// first so `show_window` can restore it. Runs on the UI thread.
///
/// Callers inside a winit `WindowEvent` dispatch (`on_close_window`,
/// `CloseRequested`) must defer this through `slint::invoke_from_event_loop`:
/// hiding mid-dispatch leaves the winit window `Arc` borrowed by the
/// dispatcher, and Slint then logs "request to hide window failed because
/// references to the window still exist".
pub fn hide_window(ui: &AppWindow) {
    let window = ui.window();
    if let Ok(mut slot) = SAVED_WINDOW_GEOM.lock() {
        *slot = Some((window.size(), window.position()));
    }
    match window.hide() {
        Ok(()) => set_window_visible(ui, false),
        Err(e) => log::warn!("tray: failed to hide the window: {e}"),
    }
}

/// Show the main window from the tray and restore its pre-hide size +
/// position. Runs on the UI thread. Handles two distinct hidden-states,
/// distinguished by whether `SAVED_WINDOW_GEOM` is populated:
///
/// 1. **`Some`**: surface was unmapped by `hide_window` (tray-hide path).
///    `Window::show()` re-maps it; the snapshotted geom is re-applied.
/// 2. **`None`**: surface is still mapped, but the window was minimized via
///    the titlebar button. Wayland exposes no client-side un-minimize:
///    `xdg_toplevel` has `set_minimized` but no inverse, and winit's
///    `focus_window` is an empty no-op on Wayland
///    (`winit/src/platform_impl/linux/wayland/window/mod.rs`). The only way
///    back from inside the app is to destroy and recreate the surface via
///    `hide()` + `show()`. Geometry is snapshotted first so it lands where
///    the minimized window was, not at the content-preferred minimum.
///
/// Both branches converge on the same `set_size` + `reschedule_geometry_restore`
/// path: Slint's first layout pass on the new winit window snaps it to the
/// layout minimum and the compositor re-places it; the synchronous resize
/// sets `has_explicit_size`, and the 16 ms-tick correction re-asserts the
/// geometry until it sticks. The correction must run from a timer (between
/// frames), not from the Resized handler — resizing mid-dispatch desyncs the
/// renderer and stretches the UI.
fn show_window(ui: &AppWindow) {
    let window = ui.window();
    let from_tray_hide = SAVED_WINDOW_GEOM.lock().ok().and_then(|mut slot| slot.take());

    let geom = if let Some(g) = from_tray_hide {
        if let Err(e) = window.show() {
            log::warn!("tray: failed to show the window: {e}");
            return;
        }
        g
    } else {
        // Button-minimize path: snapshot current geometry before the hide
        // tears the surface down; the recreate loses both otherwise.
        let g = (window.size(), window.position());
        if let Err(e) = window.hide() {
            log::warn!("tray: hide-for-restore failed: {e}");
            return;
        }
        if let Err(e) = window.show() {
            log::warn!("tray: failed to show the window: {e}");
            return;
        }
        g
    };

    let (size, position) = geom;
    window.set_size(size);
    window.set_position(position);
    log::debug!(
        "tray: restoring window geometry {}x{} @ {},{}",
        size.width,
        size.height,
        position.x,
        position.y
    );
    reschedule_geometry_restore(ui.as_weak(), size, position, RESTORE_ATTEMPTS);

    set_window_visible(ui, true);
}

/// Re-assert `size` + `position` on the window from a 16 ms single-shot timer,
/// rescheduling itself until the window's size matches (or `tries` is
/// exhausted). The winit window is recreated asynchronously after `show()`, so
/// the geometry has to be re-applied once it settles — and from a timer,
/// between frames, so the renderer stays in sync (a resize inside event
/// dispatch stretches the UI). Position is re-applied alongside size on every
/// tick; it is idempotent, and only the size has the layout-snap race that
/// drives the "settled" check.
fn reschedule_geometry_restore(
    weak: slint::Weak<AppWindow>,
    size: slint::PhysicalSize,
    position: slint::PhysicalPosition,
    tries: u8,
) {
    slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
        let Some(ui) = weak.upgrade() else { return };
        let window = ui.window();
        let current = window.size();
        if current.width == size.width && current.height == size.height {
            return; // settled
        }
        window.set_size(size);
        window.set_position(position);
        if tries > 1 {
            reschedule_geometry_restore(weak, size, position, tries - 1);
        }
    });
}

/// Wire up the system tray: create it, spawn the action receiver and the
/// state subscriber. Call once during startup, before `app.run()`.
pub fn install(spawner: &TaskSpawner, state: &AppState, ui: &AppWindow) {
    let (tx, rx) = tokio::sync::mpsc::channel::<TrayAction>(TRAY_ACTION_CHANNEL_CAP);
    spawn_action_receiver(spawner, state, ui.as_weak(), rx);

    #[cfg(target_os = "linux")]
    {
        // Linux: ksni runs on its own thread — create eagerly.
        match tray::init_tray(tx) {
            Some(linux_tray) => {
                TRAY_ACTIVE.store(true, Ordering::Relaxed);
                ui.global::<Settings>().set_tray_active(true);
                spawn_state_subscriber_linux(spawner, state, linux_tray);
            }
            None => log::info!("tray: not active — close-to-tray will fall back to quitting"),
        }
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        // Windows / macOS: `tray-icon` must be created on the UI thread once
        // the event loop is running — defer through the event loop, the same
        // way `main.rs` defers the Windows SMTC attach.
        let sinks = state.sinks.clone();
        let weak = ui.as_weak();
        if let Err(e) = slint::invoke_from_event_loop(move || {
            if tray::init_tray(tx) {
                TRAY_ACTIVE.store(true, Ordering::Relaxed);
                if let Some(ui) = weak.upgrade() {
                    ui.global::<Settings>().set_tray_active(true);
                }
                spawn_state_subscriber_local(&sinks);
            } else {
                log::info!("tray: not active — close-to-tray will fall back to quitting");
            }
        }) {
            log::warn!("tray: failed to schedule creation: {e}");
        }
    }
}

/// Tear the tray down. Must run on the UI/main thread before
/// `std::process::exit` — that call skips destructors. No-op on Linux, where
/// the `LinuxTray` is dropped by its state-subscriber task on shutdown.
pub fn shutdown() {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    tray::shutdown_tray();
}

/// Drain `TrayAction`s: playback actions into the shared `EventSink`, window
/// actions onto the UI thread.
fn spawn_action_receiver(
    spawner: &TaskSpawner,
    state: &AppState,
    ui_weak: slint::Weak<AppWindow>,
    mut rx: tokio::sync::mpsc::Receiver<TrayAction>,
) {
    let sink: Arc<dyn EventSink> = Arc::new(SlintEventSink {
        state: state.clone(),
    });
    spawner.spawn_cancellable(move |shutdown| async move {
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                maybe = rx.recv() => match maybe {
                    Some(action) => dispatch_action(action, &sink, &ui_weak),
                    None => break,
                },
            }
        }
        log::info!("tray: action receiver stopped");
    });
}

/// Route one `TrayAction`. Playback actions reuse the OS-media-controls path;
/// `ShowHideWindow` / `Quit` need the UI thread.
fn dispatch_action(
    action: TrayAction,
    sink: &Arc<dyn EventSink>,
    ui_weak: &slint::Weak<AppWindow>,
) {
    match action {
        TrayAction::PlayPause => sink.handle(PlayerEvent::PlayPause),
        TrayAction::Next => sink.handle(PlayerEvent::Next),
        TrayAction::Previous => sink.handle(PlayerEvent::Previous),
        TrayAction::ShowHideWindow => {
            let weak = ui_weak.clone();
            if let Err(e) = slint::invoke_from_event_loop(move || toggle_window(&weak)) {
                log::warn!("tray: show/hide invoke failed: {e}");
            }
        }
        TrayAction::Quit => {
            if let Err(e) = slint::invoke_from_event_loop(|| {
                if let Err(e) = slint::quit_event_loop() {
                    log::warn!("tray: quit_event_loop: {e}");
                }
            }) {
                log::warn!("tray: quit invoke failed: {e}");
            }
        }
    }
}

/// Toggle the main window between shown and hidden. Runs on the UI thread,
/// reached via `invoke_from_event_loop` (so it executes in `user_event`
/// context, not a `WindowEvent` dispatch — see `hide_window`). Uses Slint's
/// `Window::show`/`hide`, the toolkit-level hide that actually unmaps the
/// Wayland surface (what `KeePassXC` / `EasyEffects` do); winit's
/// `set_visible` is a documented no-op on Wayland.
fn toggle_window(ui_weak: &slint::Weak<AppWindow>) {
    let Some(ui) = ui_weak.upgrade() else { return };
    if WINDOW_VISIBLE.load(Ordering::Relaxed) {
        hide_window(&ui);
    } else {
        show_window(&ui);
    }
}

/// Build a `TraySnapshot` from the latest light view-model.
fn snapshot_from_vm(vm: Option<&PlayerViewModelLight>) -> TraySnapshot {
    match vm {
        Some(vm) => TraySnapshot {
            track_title: vm.current_track.as_ref().map(|t| t.title.clone()),
            track_artist: vm.current_track.as_ref().and_then(|t| t.artist.clone()),
            is_playing: vm.status == "playing",
        },
        None => TraySnapshot::default(),
    }
}

/// Linux state subscriber: a tokio task that owns the `Send` `LinuxTray` and
/// refreshes it on every view-model change. Dropping the tray when the task
/// ends (shutdown) removes the icon, so no explicit teardown is needed.
#[cfg(target_os = "linux")]
fn spawn_state_subscriber_linux(spawner: &TaskSpawner, state: &AppState, tray: tray::LinuxTray) {
    let mut rx = state.sinks.view_model.subscribe();
    spawner.spawn_cancellable(move |shutdown| async move {
        // `tray` is owned here; it is dropped (→ ksni shutdown) when the loop
        // exits. `LinuxTray::update` does a blocking D-Bus round-trip to
        // ksni's service thread; `block_in_place` hands the worker thread to
        // other tasks for its duration rather than stalling the runtime.
        // The view-model channel emits on *every* state change (volume
        // steps during a drag, seeks, queue edits), but the tray only
        // renders title / artist / play-pause — diff against the last
        // pushed snapshot so D-Bus traffic stays per-track / per-toggle.
        let mut last = snapshot_from_vm(rx.borrow_and_update().as_ref());
        tokio::task::block_in_place(|| tray.update(&last));
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                changed = rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let snapshot = snapshot_from_vm(rx.borrow_and_update().as_ref());
                    if snapshot == last {
                        continue;
                    }
                    tokio::task::block_in_place(|| tray.update(&snapshot));
                    last = snapshot;
                }
            }
        }
        log::info!("tray: state subscriber stopped");
    });
}

/// Windows / macOS state subscriber: a `slint::spawn_local` future on the UI
/// thread (the `tray-icon` handle is `!Send`). Must be spawned from the UI
/// thread — `install` calls it from inside `invoke_from_event_loop`.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn spawn_state_subscriber_local(sinks: &Arc<crate::player::event_sink::PlayerSinks>) {
    let mut rx = sinks.view_model.subscribe();
    let res = slint::spawn_local(async_compat::Compat::new(async move {
        // Diff against the last pushed snapshot — the view-model channel
        // emits per state change (volume steps, seeks), but the tray only
        // renders title / artist / play-pause.
        let mut last = snapshot_from_vm(rx.borrow_and_update().as_ref());
        tray::update_tray(&last);
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let snapshot = snapshot_from_vm(rx.borrow_and_update().as_ref());
            if snapshot == last {
                continue;
            }
            tray::update_tray(&snapshot);
            last = snapshot;
        }
        log::debug!("tray: state subscriber stopped");
    }));
    if let Err(e) = res {
        log::warn!("tray: state subscriber failed to spawn: {e}");
    }
}
