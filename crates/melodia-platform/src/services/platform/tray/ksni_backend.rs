//! The Linux tray, a `StatusNotifierItem` over [`ksni`], whose blocking API runs the service on a
//! thread of its own, so nothing here touches the event loop or the runtime.

use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, ToolTip};
use tokio::sync::mpsc;

use melodia_core::error::describe;

use super::{TrayAction, TraySnapshot};

/// Menu callbacks `try_send`, so the D-Bus thread never stalls on a full channel; it drops the
/// action instead, which no human clicking can trigger.
struct MelodiaTray {
    snapshot: TraySnapshot,
    icon: Vec<Icon>,
    action_tx: mpsc::Sender<TrayAction>,
}

impl MelodiaTray {
    fn action_item(label: &str, action: TrayAction) -> MenuItem<Self> {
        Self::action_item_enabled(label, action, true)
    }

    fn action_item_enabled(label: &str, action: TrayAction, enabled: bool) -> MenuItem<Self> {
        StandardItem {
            label: label.to_owned(),
            enabled,
            activate: Box::new(move |t: &mut Self| {
                if let Err(e) = t.action_tx.try_send(action) {
                    log::warn!("tray: dropped {action:?} (channel full): {}", describe(&e));
                }
            }),
            ..Default::default()
        }
        .into()
    }
}

impl ksni::Tray for MelodiaTray {
    fn id(&self) -> String {
        "melodia".to_owned()
    }

    fn title(&self) -> String {
        "Melodia".to_owned()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.snapshot.tooltip(),
            description: String::new(),
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    /// A left click on the icon.
    fn activate(&mut self, _x: i32, _y: i32) {
        if let Err(e) = self.action_tx.try_send(TrayAction::ShowHideWindow) {
            log::warn!("tray: dropped activate (channel full): {}", describe(&e));
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: self.snapshot.menu_track_label(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            Self::action_item(self.snapshot.play_pause_label(), TrayAction::PlayPause),
            Self::action_item_enabled("Next", TrayAction::Next, self.snapshot.has_next),
            Self::action_item_enabled("Previous", TrayAction::Previous, self.snapshot.has_previous),
            MenuItem::Separator,
            Self::action_item("Show / Hide Window", TrayAction::ShowHideWindow),
            Self::action_item("Quit Melodia", TrayAction::Quit),
        ]
    }
}

/// `Send`, so `ui::shell::tray_bridge` owns it from a background task. Dropping it removes the
/// icon.
pub struct LinuxTray {
    handle: ksni::blocking::Handle<MelodiaTray>,
    /// The panel the icon was last painted for, so a recheck that finds it unchanged sets nothing.
    on_light_panel: bool,
}

impl LinuxTray {
    pub fn update(&self, snapshot: &TraySnapshot) {
        let snapshot = snapshot.clone();
        self.handle.update(move |tray| {
            tray.snapshot = snapshot;
        });
    }

    /// Repaints the icon only where the panel changed since it was last painted.
    pub fn set_on_light_panel(&mut self, on_light_panel: bool) {
        if self.on_light_panel == on_light_panel {
            return;
        }
        let icon = icon_for(on_light_panel);
        if self.handle.update(move |tray| tray.icon = icon).is_some() {
            self.on_light_panel = on_light_panel;
        }
    }
}

impl Drop for LinuxTray {
    fn drop(&mut self) {
        // Queued to the service thread, which drops the registration and the icon with it.
        let _ = self.handle.shutdown();
    }
}

/// The network-byte-order ARGB32 ksni's `Icon` takes.
fn rgba_to_argb(mut rgba: Vec<u8>) -> Vec<u8> {
    for px in rgba.chunks_exact_mut(4) {
        px.rotate_right(1);
    }
    rgba
}

/// Empty if the asset won't decode.
fn icon_for(on_light_panel: bool) -> Vec<Icon> {
    super::decode_icon()
        .and_then(|(w, h, mut rgba)| {
            if on_light_panel {
                super::light_taskbar::paint(&mut rgba, w, h);
            }
            Some(Icon {
                width: i32::try_from(w).ok()?,
                height: i32::try_from(h).ok()?,
                data: rgba_to_argb(rgba),
            })
        })
        .map(|icon| vec![icon])
        .unwrap_or_default()
}

/// `None` with no SNI host, as on vanilla GNOME, or no D-Bus.
pub fn init(action_tx: mpsc::Sender<TrayAction>, on_light_panel: bool) -> Option<LinuxTray> {
    use ksni::blocking::TrayMethods;

    let icon = icon_for(on_light_panel);
    let tray = MelodiaTray { snapshot: TraySnapshot::default(), icon, action_tx };

    match tray.spawn() {
        Ok(handle) => {
            log::info!("System tray registered (StatusNotifierItem)");
            Some(LinuxTray { handle, on_light_panel })
        }
        Err(e) => {
            log::info!(
                "System tray unavailable ({}) — running without a tray icon. \
                 On GNOME this needs the AppIndicator extension.",
                describe(&e)
            );
            None
        }
    }
}
