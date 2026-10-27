//! What the queue's keys depend on in the two `.slint` files that hand them over and act on them,
//! neither of which fails a build or a click without it.

use melodia_testkit::{at_own_depth, block_after, code_tokens};

const QUEUE_PANEL: &str =
    include_str!("../../../../../melodia-ui/ui/views/queue-panel/queue-panel.slint");
const SHORTCUT_SCOPE: &str =
    include_str!("../../../../../melodia-ui/ui/layout/shortcut-scope.slint");

/// **The panel's hover sentinel takes the wheel its children leave.** Slint clears `has-hover` on
/// a wheel event nothing under the pointer accepts and restores it only on the next move, so
/// without the answer a click in place after a tick claims no keys. It reads as redundant, the
/// list scrolling either way.
#[test]
fn the_hover_sentinel_accepts_the_wheel_its_children_leave() {
    let panel = code_tokens(QUEUE_PANEL);
    let sentinel = block_after(&panel, "hover-catch := TouchArea");

    let answer = sentinel
        .find("scroll-event(")
        .filter(|at| at_own_depth(sentinel, *at))
        .map(|at| block_after(&sentinel[at..], "scroll-event(").trim());

    assert_eq!(answer, Some("return accept;"), "the sentinel's own wheel handler accepts");
}

/// **Delete and Backspace remove the queue's picks below the dialog guard, and only on the
/// queue's keys.** Both are destructive and neither reaches the root from a field, so above the
/// guard a confirm dialog's Backspace would empty the queue behind it, and off `queue-keys` a
/// Delete meant for the page beside the docked panel would take the panel's picks.
#[test]
fn delete_and_backspace_act_below_the_dialog_guard_and_only_on_the_queues_keys() {
    let keys = code_tokens(SHORTCUT_SCOPE);

    let order = (
        keys.find("if (Dialog.open || Onboarding.open) {"),
        keys.find("if ((ev.text == Key.Delete || ev.text == Key.Backspace) && root.queue-keys"),
    );

    assert!(
        matches!(order, (Some(guard), Some(arm)) if guard < arm),
        "the removal arm reads `queue-keys` and sits below the guard: {order:?}"
    );
    assert_eq!(
        (keys.matches("Key.Delete").count(), keys.matches("Key.Backspace").count()),
        (1, 1),
        "a second arm on either key answers outside the one gated above"
    );
}
