use super::CRASH_TOAST_KIND;

const DISPATCHER: &str = include_str!("../../../melodia-ui/ui/globals/updater.slint");
const SECTION: &str =
    include_str!("../../../melodia-ui/ui/views/settings/diagnostics-section.slint");

/// The routing key Rust pushes and the branch that answers it are one string
/// split across two languages, so nothing but a scan can hold them together.
/// A mismatch is silent in the worst way: `notification-stack.slint` gates the
/// action button on a non-empty label alone, so the toast renders complete and
/// the click falls off the end of a dispatcher that has no `else`.
#[test]
fn the_toast_kind_matches_its_dispatcher_branch() {
    let branch = format!("kind == \"{CRASH_TOAST_KIND}\"");
    assert!(
        DISPATCHER.contains(&branch),
        "no `{branch}` branch in the Notifications.action dispatcher — the crash \
         toast's action button would render and do nothing"
    );
}

/// The branch is only worth having if it reaches the callback the Diagnostics
/// card's own button reaches; otherwise the toast opens nothing.
#[test]
fn the_crash_branch_and_the_card_open_the_same_folder() {
    let (_, after_branch) = DISPATCHER
        .split_once(&format!("kind == \"{CRASH_TOAST_KIND}\""))
        .unwrap_or_default();
    let body = after_branch.split_once('}').map(|(body, _)| body).unwrap_or_default();

    assert!(
        body.contains("Settings.open-log-folder()"),
        "the crash-report branch must call the same callback the card's button does"
    );
    assert!(
        SECTION.contains("Settings.open-log-folder()"),
        "the Diagnostics card no longer wires `open-log-folder` — the toast now \
         points somewhere the card doesn't"
    );
}
