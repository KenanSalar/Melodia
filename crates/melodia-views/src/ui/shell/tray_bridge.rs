//! The tray's two directions, its actions in and the player's state out, plus the close-to-tray
//! state a close and the tray's Show/Hide read.

#[cfg(target_os = "linux")]
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

use crate::ui::shell::event_sink::SlintEventSink;
use crate::ui::window_chrome::geometry::{self, Reshow};
use melodia_app::services::updater;
use melodia_app::state::AppState;
#[cfg(target_os = "linux")]
use melodia_app::state::Signal;
use melodia_app::tasks::TaskSpawner;
use melodia_core::error::describe;
use melodia_engine::player::engine::event_sink::{EventSink, PlayerEvent};
use melodia_engine::player::engine::state::PlayerViewModelLight;
use melodia_platform::services::platform::tray::{
    self, TRAY_ACTION_CHANNEL_CAP, TrayAction, TraySnapshot,
};
#[cfg(target_os = "linux")]
use melodia_platform::services::platform::{desktop, system_theme};
use melodia_ui::{AppWindow, Settings, Visualizer};

/// Mirrors `settings.tray.close_to_tray`, so a close reads it without touching disk.
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(false);

/// `winit::Window::is_visible()` is `None` on Wayland.
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

/// Close-to-tray falls back to quitting without a tray, or it would strand a hidden window.
static TRAY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set by a tray hide and taken by the next show, which without it is restoring from a minimize.
static HIDDEN_TO_TRAY: AtomicBool = AtomicBool::new(false);

/// How many timer ticks a rebuilt window's size is re-asserted for.
const RESTORE_ATTEMPTS: u8 = 8;

/// Asks the Linux subscriber, which owns the tray, to re-read the panel under it.
#[cfg(target_os = "linux")]
static PANEL_RECHECK: LazyLock<Signal> = LazyLock::new(Signal::new);

pub fn set_close_to_tray(on: bool) {
    CLOSE_TO_TRAY.store(on, Ordering::Relaxed);
}

pub fn should_hide_to_tray() -> bool {
    CLOSE_TO_TRAY.load(Ordering::Relaxed)
        && TRAY_ACTIVE.load(Ordering::Relaxed)
        && !updater::installer_running()
}

/// Moves the visibility shadow and `Visualizer.window-shown` together, the strip's Timer gating on
/// the Slint half while its tick reads the atomic. Out of step, the next tray click hides an
/// already-minimized window and restoring takes two.
pub fn set_window_visible(ui: &AppWindow, visible: bool) {
    WINDOW_VISIBLE.store(visible, Ordering::Relaxed);
    let viz = ui.global::<Visualizer>();
    viz.set_window_shown(visible);
    if visible {
        // It may have gone dormant off screen; this is the notice.
        viz.set_dormant(false);
    } else {
        // The tap can't wait for the strip's next tick: `window-shown` gates that Timer
        // too, so the call above may have just stopped it.
        viz.invoke_window_hidden();
    }
}

/// For UI work running off a `Timer` while hidden, the loop surviving a close-to-tray hide. A
/// cheap gate: Wayland says nothing of a minimize, which `ui::visualizer::pulse` covers.
#[must_use]
pub fn is_window_visible() -> bool {
    WINDOW_VISIBLE.load(Ordering::Relaxed)
}

/// Hides the window to the tray. A caller inside a winit `WindowEvent` dispatch posts it: there the
/// dispatcher still borrows the window and Slint logs "references to the window still exist".
pub fn hide_window(ui: &AppWindow) {
    match ui.window().hide() {
        Ok(()) => {
            HIDDEN_TO_TRAY.store(true, Ordering::Relaxed);
            set_window_visible(ui, false);
        }
        Err(e) => log::warn!("tray: failed to hide the window: {}", describe(&e)),
    }
}

/// Shows the window again. Restoring from a minimize goes through `hide()` first, Wayland having
/// no client-side un-minimize (`set_minimized` has no inverse and `focus_window` is a no-op).
///
/// Only a Wayland hide tears the window down, so only there is it rebuilt. A window that survived
/// keeps its own size, position and maximized state, and writing its position back would move it
/// by the frame's height on X11.
fn show_window(ui: &AppWindow) {
    let window = ui.window();
    if !HIDDEN_TO_TRAY.swap(false, Ordering::Relaxed)
        && let Err(e) = window.hide()
    {
        log::warn!("tray: hide-for-restore failed: {}", describe(&e));
        return;
    }

    let recreated = window.with_winit_window(|_| ()).is_none();
    let reshow = if recreated { geometry::reshow() } else { None };
    if let Err(e) = window.show() {
        log::warn!("tray: failed to show the window: {}", describe(&e));
        return;
    }
    if let Some(reshow) = reshow {
        rebuild(ui, reshow);
    }
    #[cfg(target_os = "linux")]
    if !recreated {
        activate_for_user(ui);
    }

    set_window_visible(ui, true);
}

/// On X11 a window manager focuses only what is new or what asks, and a minimized window mapped
/// again is neither. The tray click is the user's, so the window asks as a tray icon does.
#[cfg(target_os = "linux")]
fn activate_for_user(ui: &AppWindow) {
    use slint::winit_030::winit::platform::x11::WindowExtX11;

    ui.window().with_winit_window(WindowExtX11::activate_for_user);
}

/// Sizes a window Wayland re-created before the event loop builds it: the size lands in its
/// attributes, against Slint's first layout snapping it to the minimum, and is the one
/// un-maximizing returns to. A restored window's size is re-asserted between frames too, resizing
/// mid-dispatch desyncing the renderer.
fn rebuild(ui: &AppWindow, reshow: Reshow) {
    let window = ui.window();
    log::debug!("tray: rebuilding the window as {reshow:?}");
    match reshow {
        Reshow::Restored(size) => {
            let size = size.to_physical(window.scale_factor());
            window.set_size(size);
            reschedule_size_restore(ui.as_weak(), size, RESTORE_ATTEMPTS);
        }
        Reshow::Maximized { restore_size } => {
            window.set_size(restore_size);
            maximize_once_built(ui.as_weak());
        }
    }
}

/// Through winit, once the event loop has built the window: Slint's own `set_maximized` is
/// swallowed, its cached flag still `true` from before the hide.
fn maximize_once_built(weak: slint::Weak<AppWindow>) {
    if let Err(e) = slint::spawn_local(async move {
        let Some(ui) = weak.upgrade() else { return };
        match ui.window().winit_window().await {
            Ok(window) => window.set_maximized(true),
            Err(e) => log::warn!("tray: re-maximize: {}", describe(&e)),
        }
    }) {
        log::warn!("tray: re-maximize: schedule: {}", describe(&e));
    }
}

/// Bring the window to the user, whatever put it out of reach. Hidden, by the tray or a minimize,
/// goes through [`show_window`], the only path that tells the two apart and rebuilds a window
/// Wayland re-created. Merely buried is the window server's business, and Wayland gives a client
/// no say.
pub fn raise_window(ui: &AppWindow) {
    use slint::winit_030::winit::window::Window as WinitWindow;

    if !is_window_visible() {
        show_window(ui);
        return;
    }
    ui.window().with_winit_window(WinitWindow::focus_window);
}

/// Re-asserts the size each tick until it sticks or `tries` runs out, the winit window being
/// recreated asynchronously after `show()`.
fn reschedule_size_restore(weak: slint::Weak<AppWindow>, size: slint::PhysicalSize, tries: u8) {
    slint::Timer::single_shot(std::time::Duration::from_millis(16), move || {
        let Some(ui) = weak.upgrade() else { return };
        let window = ui.window();
        if window.size() == size {
            return;
        }
        window.set_size(size);
        if tries > 1 {
            reschedule_size_restore(weak, size, tries - 1);
        }
    });
}

/// Creates the tray and its two tasks, once, before `app.show()`.
pub fn install(spawner: &TaskSpawner, state: &AppState, ui: &AppWindow) {
    let (tx, rx) = tokio::sync::mpsc::channel::<TrayAction>(TRAY_ACTION_CHANNEL_CAP);
    spawn_action_receiver(spawner, state, ui.as_weak(), rx);

    #[cfg(target_os = "linux")]
    {
        // ksni runs its own thread, so it can be created now.
        match tray::init_tray(tx, on_light_panel()) {
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
        // `tray-icon` wants the UI thread with the event loop already running, so defer
        // the way boot defers the SMTC attach.
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
            log::warn!("tray: failed to schedule creation: {}", describe(&e));
        }
    }
}

/// Repaint the tray icon for the taskbar's or panel's current mode, on the UI thread. Neither OS
/// says when only that surface moves (the Windows mode, a Plasma style), so a focus gain asks, as
/// the window border's colour does.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub fn refresh_icon() {
    #[cfg(target_os = "windows")]
    tray::refresh_tray_icon();
    #[cfg(target_os = "linux")]
    PANEL_RECHECK.bump();
}

/// Whether the tray sits on a light panel. Gated on the session rather than on Plasma's files, which
/// another desktop can leave behind and which describe no panel there, so elsewhere the icon keeps
/// the asset's colours.
#[cfg(target_os = "linux")]
fn on_light_panel() -> bool {
    desktop::is_kde_desktop() && system_theme::plasma_panel_is_light()
}

/// Tear the tray down, on the UI thread and before `process::exit` skips every
/// destructor. No-op on Linux, whose `LinuxTray` its subscriber drops.
pub fn shutdown() {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    tray::shutdown_tray();
}

fn spawn_action_receiver(
    spawner: &TaskSpawner,
    state: &AppState,
    ui_weak: slint::Weak<AppWindow>,
    mut rx: tokio::sync::mpsc::Receiver<TrayAction>,
) {
    let sink = SlintEventSink { state: state.clone() };
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

/// Playback actions reuse the OS media controls' sink; the window ones need the UI thread.
fn dispatch_action(action: TrayAction, sink: &dyn EventSink, ui_weak: &slint::Weak<AppWindow>) {
    match action {
        TrayAction::PlayPause => sink.handle(PlayerEvent::PlayPause),
        TrayAction::Next => sink.handle(PlayerEvent::Next),
        TrayAction::Previous => sink.handle(PlayerEvent::Previous),
        TrayAction::ShowHideWindow => {
            let weak = ui_weak.clone();
            if let Err(e) = slint::invoke_from_event_loop(move || toggle_window(&weak)) {
                log::warn!("tray: show/hide invoke failed: {}", describe(&e));
            }
        }
        TrayAction::Quit => {
            if let Err(e) = slint::invoke_from_event_loop(|| {
                if let Err(e) = slint::quit_event_loop() {
                    log::warn!("tray: quit_event_loop: {}", describe(&e));
                }
            }) {
                log::warn!("tray: quit invoke failed: {}", describe(&e));
            }
        }
    }
}

/// Posted, so it runs outside any `WindowEvent` dispatch (see `hide_window`). Slint's
/// `show`/`hide` unmaps a Wayland surface, where winit's `set_visible` is a no-op.
fn toggle_window(ui_weak: &slint::Weak<AppWindow>) {
    let Some(ui) = ui_weak.upgrade() else { return };
    if WINDOW_VISIBLE.load(Ordering::Relaxed) {
        hide_window(&ui);
    } else {
        show_window(&ui);
    }
}

/// Through `source()`, so a station reaches the tooltip without this file learning what one is.
fn snapshot_from_vm(vm: Option<&PlayerViewModelLight>) -> TraySnapshot {
    let Some(vm) = vm else {
        return TraySnapshot::default();
    };
    let source = vm.source();
    TraySnapshot {
        track_title: source.as_ref().map(|s| s.title.to_owned()),
        track_artist: source.as_ref().and_then(|s| s.secondary.map(str::to_owned)),
        is_playing: vm.status == "playing",
        has_next: vm.has_next,
        has_previous: vm.has_previous,
    }
}

/// Owns the `Send` `LinuxTray`, whose drop at the task's end removes the icon.
#[cfg(target_os = "linux")]
fn spawn_state_subscriber_linux(
    spawner: &TaskSpawner,
    state: &AppState,
    mut tray: tray::LinuxTray,
) {
    let mut rx = state.sinks.view_model.subscribe();
    let mut panel_recheck = PANEL_RECHECK.subscribe();
    spawner.spawn_cancellable(move |shutdown| async move {
        // `update` is a blocking D-Bus round trip, hence `block_in_place`. The channel emits on
        // every state change, so a snapshot unchanged from the last is skipped.
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
                Ok(()) = panel_recheck.changed() => {
                    tokio::task::block_in_place(|| tray.set_on_light_panel(on_light_panel()));
                }
            }
        }
        log::info!("tray: state subscriber stopped");
    });
}

/// On the UI thread, the `tray-icon` handle being `!Send`; `install` spawns it from there.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn spawn_state_subscriber_local(sinks: &melodia_engine::player::engine::event_sink::PlayerSinks) {
    let mut rx = sinks.view_model.subscribe();
    let res = slint::spawn_local(async_compat::Compat::new(async move {
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
        log::warn!("tray: state subscriber failed to spawn: {}", describe(&e));
    }
}
