//! The system tray: `ksni` on Linux, a `StatusNotifierItem` over D-Bus with no GTK, and
//! `tray_icon` on Windows and macOS, behind one façade.
//!
//! A vanilla GNOME session has no `StatusNotifier` host, so there `init_tray` answers `None`, as on
//! any session without a working tray, and every tray action still has an in-app control.
//!
//! The labels are English-only: `@tr` resolves only literals inside `.slint`, and this layer may
//! not depend on Slint.

#[cfg(target_os = "linux")]
mod ksni_backend;
mod light_taskbar;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod tray_icon_backend;

#[cfg(target_os = "linux")]
pub use ksni_backend::{LinuxTray, init as init_tray};
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use tray_icon_backend::{
    init as init_tray, refresh_icon as refresh_tray_icon, shutdown as shutdown_tray,
    update as update_tray,
};

/// The tray's callback thread `try_send`s into it, and a human clicking can't outrun this many.
pub const TRAY_ACTION_CHANNEL_CAP: usize = 16;

/// A menu action emitted by the tray, drained by `ui::shell::tray_bridge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    PlayPause,
    Next,
    Previous,
    ShowHideWindow,
    Quit,
}

/// What the tray renders. `PartialEq` so an emit that changed none of it, a volume step or a seek,
/// skips the blocking round trip to the tray.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraySnapshot {
    /// A track's title, or the song a station announced.
    pub track_title: Option<String>,
    /// A track's artist, or the station's name.
    pub track_artist: Option<String>,
    pub is_playing: bool,
    /// Both false for a live stream, so the menu rows follow rather than offering a no-op.
    pub has_next: bool,
    pub has_previous: bool,
}

impl TraySnapshot {
    /// `"Title — Artist"`, `"Title"`, or `"Melodia"`.
    fn tooltip(&self) -> String {
        match (&self.track_title, &self.track_artist) {
            (Some(title), Some(artist)) if !artist.is_empty() => {
                format!("{title} — {artist}")
            }
            (Some(title), _) => title.clone(),
            (None, _) => "Melodia".to_owned(),
        }
    }

    /// The disabled track-name row at the top of the menu.
    fn menu_track_label(&self) -> String {
        self.track_title.clone().unwrap_or_else(|| "Melodia".to_owned())
    }

    fn play_pause_label(&self) -> &'static str {
        if self.is_playing { "Pause" } else { "Play" }
    }
}

/// Rasterised from `crates/melodia-ui/ui/assets/icons/logo-without-background.svg`.
const TRAY_PNG: &[u8] = include_bytes!("../../../../../../assets/icons/tray.png");

/// `(width, height, rgba8)`, or `None` where the asset won't decode, a tray without an icon
/// beating no tray.
fn decode_icon() -> Option<(u32, u32, Vec<u8>)> {
    match image::load_from_memory_with_format(TRAY_PNG, image::ImageFormat::Png) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            Some((w, h, rgba.into_raw()))
        }
        Err(e) => {
            log::warn!(
                "tray: failed to decode embedded icon: {}",
                melodia_core::error::describe(&e)
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/tray_tests.rs"]
mod tests;
