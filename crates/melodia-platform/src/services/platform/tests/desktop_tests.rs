use melodia_testkit::with_env_var;

use super::*;

#[test]
fn corner_radius_by_desktop_environment() {
    let radius_under = |desktop| with_env_var("XDG_CURRENT_DESKTOP", desktop, get_os_corner_radius);

    assert_eq!(radius_under(Some("GNOME")), 15, "GNOME should return 15");
    assert_eq!(radius_under(Some("ubuntu:GNOME")), 15, "ubuntu:GNOME should return 15");
    assert_eq!(radius_under(Some("KDE")), 6, "KDE should return 6");
    assert_eq!(radius_under(Some("i3")), 6, "unknown DE should return 6");
    assert_eq!(radius_under(None), 6, "missing env should return 6");
}
