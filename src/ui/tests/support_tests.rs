use super::{KOFI_URL, SUPPORT_TOAST_KIND};

const DISPATCHER: &str = include_str!("../../../melodia-ui/ui/globals/updater.slint");
const SECTION: &str = include_str!("../../../melodia-ui/ui/views/settings/about-section.slint");

/// The routing key Rust pushes and the branch that answers it are one string split
/// across two languages, so nothing but a scan can hold them together. A mismatch is
/// silent in the worst way: `notification-stack.slint` gates the action button on a
/// non-empty label alone, so the toast renders complete and the click falls off the end
/// of a dispatcher that has no `else`.
#[test]
fn the_toast_kind_matches_its_dispatcher_branch() {
    let branch = format!("kind == \"{SUPPORT_TOAST_KIND}\"");
    assert!(
        DISPATCHER.contains(&branch),
        "no `{branch}` branch in the Notifications.action dispatcher — the support \
         toast's action button would render and do nothing"
    );
}

/// The branch is only worth having if it reaches the callback the About card's own
/// button reaches; otherwise the toast opens nothing.
#[test]
fn the_support_branch_and_the_card_open_the_same_page() {
    let (_, after_branch) =
        DISPATCHER.split_once(&format!("kind == \"{SUPPORT_TOAST_KIND}\"")).unwrap_or_default();
    let body = after_branch.split_once('}').map(|(body, _)| body).unwrap_or_default();

    assert!(
        body.contains("Settings.open-kofi()"),
        "the support branch must call the same callback the card's button does"
    );
    assert!(
        SECTION.contains("Settings.open-kofi()"),
        "the About card no longer wires `open-kofi` — the toast now points somewhere \
         the card doesn't"
    );
}

/// The mark rides in the button rather than in the icon font, so it is the one control
/// in the card that can't be checked by reading a ligature name. `colorize` would
/// flatten a three-colour trade mark onto one brush, which is both the wrong rendering
/// and the wrong thing to do to someone else's logo.
#[test]
fn the_kofi_mark_is_an_unrecoloured_image() {
    let (_, after) = SECTION.split_once("leading-image:").unwrap_or_default();
    let mount = after.split_once(';').map(|(value, _)| value).unwrap_or_default();

    assert!(
        mount.contains("assets/icons/kofi-symbol.svg"),
        "the About card's support button no longer mounts the Ko-fi mark"
    );
    assert!(!SECTION.contains("colorize"), "the Ko-fi mark must keep its own colours");
}

/// A typo here is a support link that quietly goes nowhere, and nothing else in the
/// tree names this page.
#[test]
fn the_kofi_url_is_the_project_page() {
    assert_eq!(KOFI_URL, "https://ko-fi.com/kenansalar");
}
