//! Windows / macOS tray backend, built on [`tray_icon`].
//!
//! `tray_icon::TrayIcon` and the `muda` menu items are `!Send` — they must be
//! created, updated, and dropped on the thread that owns the event loop (the
//! Slint UI / main thread). They are therefore parked in a `thread_local`
//! rather than handed back as a value: `update` and `shutdown` are free
//! functions the UI thread calls. `shutdown` must run before `main`'s
//! `std::process::exit(0)` — that call skips destructors, and a leaked
//! `TrayIcon` lingers as a ghost entry in the Windows notification area.

use std::cell::RefCell;

use tokio::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use super::{TrayAction, TraySnapshot};

// Stable menu-item ids — `muda` delivers `MenuEvent { id }` globally, so the
// event handler routes by id.
const ID_PLAY_PAUSE: &str = "melodia.tray.playpause";
const ID_NEXT: &str = "melodia.tray.next";
const ID_PREV: &str = "melodia.tray.previous";
const ID_SHOW_HIDE: &str = "melodia.tray.showhide";
const ID_QUIT: &str = "melodia.tray.quit";

/// UI-thread-owned tray state. The four menu items are retained so `update` can rewrite the
/// labels and follow the transport's own enabled state.
struct TrayState {
    tray: TrayIcon,
    track_item: MenuItem,
    play_pause_item: MenuItem,
    next_item: MenuItem,
    prev_item: MenuItem,
    /// The taskbar the icon was last painted for, so a recheck that finds it unchanged sets nothing.
    on_light_taskbar: bool,
}

thread_local! {
    /// The live tray, owned by the Slint UI thread for its whole lifetime.
    static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

/// Create the tray icon. MUST be called on the Slint UI thread once the
/// event loop is running (`tray_icon` on macOS needs the `NSApplication` up;
/// on Windows the icon's hidden window is pumped by the winit message loop).
/// Returns `false` when the platform refused to create it — the app then
/// runs without a tray.
pub fn init(action_tx: mpsc::Sender<TrayAction>) -> bool {
    let menu = Menu::new();
    let track_item = MenuItem::new("Melodia", false, None);
    let play_pause_item = MenuItem::with_id(ID_PLAY_PAUSE, "Play", true, None);
    // Both start disabled, matching `TraySnapshot::default()`: nothing is playing yet, so there
    // is nowhere to skip to until the first snapshot lands.
    let next_item = MenuItem::with_id(ID_NEXT, "Next", false, None);
    let prev_item = MenuItem::with_id(ID_PREV, "Previous", false, None);
    let show_hide_item = MenuItem::with_id(ID_SHOW_HIDE, "Show / Hide Window", true, None);
    let quit_item = MenuItem::with_id(ID_QUIT, "Quit Melodia", true, None);
    let sep_a = PredefinedMenuItem::separator();
    let sep_b = PredefinedMenuItem::separator();

    if let Err(e) = menu.append_items(&[
        &track_item,
        &sep_a,
        &play_pause_item,
        &next_item,
        &prev_item,
        &sep_b,
        &show_hide_item,
        &quit_item,
    ]) {
        log::warn!("tray: failed to build menu: {e}");
        return false;
    }

    let on_light_taskbar = taskbar_is_light();
    let mut builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Melodia");
    if let Some(icon) = icon_for(on_light_taskbar) {
        builder = builder.with_icon(icon);
    }

    let tray = match builder.build() {
        Ok(tray) => tray,
        Err(e) => {
            log::warn!("System tray unavailable: {e}");
            return false;
        }
    };

    // Route `muda` menu clicks into the action channel. The handler runs on
    // whichever thread pumps the platform events; `try_send` is non-blocking.
    MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
        let action = match ev.id.as_ref() {
            ID_PLAY_PAUSE => TrayAction::PlayPause,
            ID_NEXT => TrayAction::Next,
            ID_PREV => TrayAction::Previous,
            ID_SHOW_HIDE => TrayAction::ShowHideWindow,
            ID_QUIT => TrayAction::Quit,
            _ => return,
        };
        if let Err(e) = action_tx.try_send(action) {
            log::warn!("tray: dropped {action:?} (channel full): {e}");
        }
    }));

    TRAY.with_borrow_mut(|slot| {
        *slot = Some(TrayState {
            tray,
            track_item,
            play_pause_item,
            next_item,
            prev_item,
            on_light_taskbar,
        });
    });
    log::info!("System tray registered");
    true
}

/// Repaint the icon if the taskbar changed mode since it was last painted. Must be called on the UI
/// thread. No-op if the tray was never created or has been shut down.
pub fn refresh_icon() {
    TRAY.with_borrow_mut(|slot| {
        let Some(state) = slot else { return };
        let on_light_taskbar = taskbar_is_light();
        if state.on_light_taskbar == on_light_taskbar {
            return;
        }
        let Some(icon) = icon_for(on_light_taskbar) else { return };
        match state.tray.set_icon(Some(icon)) {
            Ok(()) => state.on_light_taskbar = on_light_taskbar,
            Err(e) => log::debug!("tray: set_icon failed: {e}"),
        }
    });
}

/// The embedded icon, painted for the taskbar it sits on.
fn icon_for(on_light_taskbar: bool) -> Option<Icon> {
    let (width, height, mut rgba) = super::decode_icon()?;
    if on_light_taskbar {
        super::light_taskbar::paint(&mut rgba, width, height);
    }
    match Icon::from_rgba(rgba, width, height) {
        Ok(icon) => Some(icon),
        Err(e) => {
            log::warn!("tray: embedded icon rejected: {e}");
            None
        }
    }
}

#[cfg(target_os = "windows")]
fn taskbar_is_light() -> bool {
    crate::services::platform::color_mode::taskbar_theme() == "light"
}

/// Nothing here reads the macOS menu bar's appearance, so the icon keeps the asset's colours.
#[cfg(target_os = "macos")]
fn taskbar_is_light() -> bool {
    false
}

/// Push a fresh snapshot — relabels the play/pause + track rows, follows the transport on the
/// skip rows, and rewrites the tooltip. Must be called on the UI thread. No-op if the tray was
/// never created or has been shut down.
pub fn update(snapshot: &TraySnapshot) {
    TRAY.with_borrow(|slot| {
        let Some(state) = slot else { return };
        state.track_item.set_text(snapshot.menu_track_label());
        state.play_pause_item.set_text(snapshot.play_pause_label());
        state.next_item.set_enabled(snapshot.has_next);
        state.prev_item.set_enabled(snapshot.has_previous);
        if let Err(e) = state.tray.set_tooltip(Some(snapshot.tooltip())) {
            log::debug!("tray: set_tooltip failed: {e}");
        }
    });
}

/// Drop the tray icon and clear the global menu-event handler. Must be called
/// on the UI thread before `std::process::exit`.
pub fn shutdown() {
    if TRAY.with_borrow_mut(std::option::Option::take).is_some() {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        log::info!("System tray removed");
    }
}
