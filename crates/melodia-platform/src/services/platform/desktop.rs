//! Which desktop the session runs under, asked by the first-launch defaults and by the
//! capabilities that follow the host.

/// The Linux desktop a fresh install dresses itself for. Windows and macOS read as `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostDesktop {
    Kde,
    Gnome,
    Other,
}

#[cfg(target_os = "linux")]
#[must_use]
pub fn host_desktop() -> HostDesktop {
    std::env::var("XDG_CURRENT_DESKTOP").map_or(HostDesktop::Other, |value| desktop_named(&value))
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn host_desktop() -> HostDesktop {
    HostDesktop::Other
}

/// `XDG_CURRENT_DESKTOP` is a colon-separated list (`ubuntu:GNOME`), and the first name Melodia
/// knows decides.
#[cfg(target_os = "linux")]
fn desktop_named(value: &str) -> HostDesktop {
    value
        .split(':')
        .find_map(|segment| match segment {
            "KDE" => Some(HostDesktop::Kde),
            "GNOME" => Some(HostDesktop::Gnome),
            _ => None,
        })
        .unwrap_or(HostDesktop::Other)
}

/// Whether the active session is KDE Plasma. Seeds
/// `LayoutFlags.match_unfocused_to_system_bg` and hides the matching Appearance
/// row elsewhere, the behaviour it drives being KDE's own.
pub fn is_kde_desktop() -> bool {
    host_desktop() == HostDesktop::Kde
}

/// The host desktop's window corner radius, in pixels. Values mirror the chip
/// presets in the Appearance section, so a first launch lights up the chip
/// matching the environment.
#[must_use]
pub fn get_os_corner_radius() -> u32 {
    #[cfg(target_os = "macos")]
    {
        10
    }
    #[cfg(target_os = "windows")]
    {
        8
    }
    #[cfg(target_os = "linux")]
    {
        match host_desktop() {
            HostDesktop::Gnome => 15,
            HostDesktop::Kde | HostDesktop::Other => 6,
        }
    }
}
