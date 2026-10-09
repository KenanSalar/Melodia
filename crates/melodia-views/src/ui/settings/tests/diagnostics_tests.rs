const ROUTER: &str = include_str!("../../shell/notifications.rs");
const SECTION: &str =
    include_str!("../../../../../melodia-ui/ui/views/settings/diagnostics-section.slint");

/// The toast's action is only worth having if it reaches the callback the Diagnostics
/// card's own button reaches; otherwise the toast opens nothing.
#[test]
fn the_crash_toast_and_the_card_open_the_same_folder() {
    let (_, after_arm) = ROUTER.split_once("NotificationKind::CrashReport =>").unwrap_or_default();
    let arm = after_arm.split_once('\n').map(|(arm, _)| arm).unwrap_or_default();

    assert!(
        arm.contains("invoke_open_log_folder()"),
        "the crash-report arm must call the same callback the card's button does"
    );
    assert!(
        SECTION.contains("Settings.open-log-folder()"),
        "the Diagnostics card no longer wires `open-log-folder` — the toast now \
         points somewhere the card doesn't"
    );
}
