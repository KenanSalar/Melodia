//! The Windows and macOS tray, over [`tray_icon`].
//!
//! `TrayIcon` and the `muda` items are `!Send`, living and dying on the UI thread, so they sit in
//! a `thread_local` behind free functions. `shutdown` has to run before `main`'s
//! `process::exit(0)`, which skips destructors and would leave a ghost icon in the notification
//! area.

use std::cell::RefCell;

use tokio::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use melodia_core::error::describe;

use super::{TrayAction, TraySnapshot};

// `muda` delivers every `MenuEvent` to one global handler, which routes by these.
const ID_PLAY_PAUSE: &str = "melodia.tray.playpause";
const ID_NEXT: &str = "melodia.tray.next";
const ID_PREV: &str = "melodia.tray.previous";
const ID_SHOW_HIDE: &str = "melodia.tray.showhide";
const ID_QUIT: &str = "melodia.tray.quit";

/// The four items are kept so `update` can relabel them and follow the transport.
struct TrayState {
    tray: TrayIcon,
    track_item: MenuItem,
    play_pause_item: MenuItem,
    next_item: MenuItem,
    prev_item: MenuItem,
    /// What the icon was last painted for, so a recheck finding it unchanged sets nothing.
    on_light_taskbar: bool,
}

thread_local! {
    static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

/// On the UI thread once the event loop runs: macOS needs the `NSApplication` up, and on Windows
/// the icon's hidden window is pumped by winit's loop. `false` where the platform refused it.
pub fn init(action_tx: mpsc::Sender<TrayAction>) -> bool {
    let menu = Menu::new();
    let track_item = MenuItem::new("Melodia", false, None);
    let play_pause_item = MenuItem::with_id(ID_PLAY_PAUSE, "Play", true, None);
    // Disabled, matching `TraySnapshot::default()` until the first snapshot lands.
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
        log::warn!("tray: failed to build menu: {}", describe(&e));
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
            log::warn!("System tray unavailable: {}", describe(&e));
            return false;
        }
    };

    // Runs on whichever thread pumps the platform events, hence `try_send`.
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
            log::warn!("tray: dropped {action:?} (channel full): {}", describe(&e));
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

/// Repaints only where the taskbar changed mode since the last paint. UI thread only.
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
            Err(e) => log::debug!("tray: set_icon failed: {}", describe(&e)),
        }
    });
}

fn icon_for(on_light_taskbar: bool) -> Option<Icon> {
    let (width, height, mut rgba) = super::decode_icon()?;
    if on_light_taskbar {
        super::light_taskbar::paint(&mut rgba, width, height);
    }
    match Icon::from_rgba(rgba, width, height) {
        Ok(icon) => Some(icon),
        Err(e) => {
            log::warn!("tray: embedded icon rejected: {}", describe(&e));
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

/// UI thread only.
pub fn update(snapshot: &TraySnapshot) {
    TRAY.with_borrow(|slot| {
        let Some(state) = slot else { return };
        state.track_item.set_text(snapshot.menu_track_label());
        state.play_pause_item.set_text(snapshot.play_pause_label());
        state.next_item.set_enabled(snapshot.has_next);
        state.prev_item.set_enabled(snapshot.has_previous);
        if let Err(e) = state.tray.set_tooltip(Some(snapshot.tooltip())) {
            log::debug!("tray: set_tooltip failed: {}", describe(&e));
        }
    });
}

/// UI thread only, before `process::exit`. The menu handler stays installed: `muda`'s slot takes
/// one value per process, and with the menu gone nothing reaches it.
pub fn shutdown() {
    if TRAY.with_borrow_mut(std::option::Option::take).is_some() {
        log::info!("System tray removed");
    }
}
