//! The app id against the desktop entry it names, an agreement no compiler sees. Every Linux
//! format installs that one entry.

use super::APP_ID;

const DESKTOP_ENTRY: &str =
    include_str!("../../../../packaging/com.github.kenansalar.melodia.desktop");
const METAINFO: &str =
    include_str!("../../../../packaging/com.github.kenansalar.melodia.metainfo.xml");

/// X11 matches a window to its entry by the `WM_CLASS` class, which Slint fills with the app id.
/// Matched to none, KDE gives the window a taskbar entry apart from its launcher's.
#[test]
fn the_desktop_entry_matches_the_app_id_on_x11() {
    let wm_class = DESKTOP_ENTRY.lines().find_map(|line| line.strip_prefix("StartupWMClass="));

    assert_eq!(wm_class, Some(APP_ID));
}

/// Wayland matches a window's `app_id` to the desktop file of that basename, the desktop id the
/// metainfo declares. Matched to none, the compositor shows a generic icon.
#[test]
fn the_desktop_entry_matches_the_app_id_on_wayland() {
    let desktop_id = METAINFO
        .split_once(r#"<launchable type="desktop-id">"#)
        .and_then(|(_, rest)| rest.split_once("</launchable>"))
        .map(|(id, _)| id);

    assert_eq!(desktop_id, Some(format!("{APP_ID}.desktop").as_str()));
}
