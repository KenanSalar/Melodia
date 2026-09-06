//! What the first-run card's `.slint` half can break without failing a build.
//!
//! Both properties here are invisible to review and to every other walk: one panics at runtime on
//! the *second* open, the other silently reorders which surface Escape reaches first.

use melodia_testkit::{UI_DIR, strip_line_comments};

/// The card's own components. A floor rather than a list, so a sixth panel is covered on arrival,
/// and low enough that deleting one file is an ordinary edit rather than a failure here.
const MIN_ONBOARDING_SOURCES: usize = 5;

const APP_WINDOW: &str = include_str!("../../melodia-ui/ui/app-window.slint");
const SHORTCUT_SCOPE: &str = include_str!("../../melodia-ui/ui/layout/shortcut-scope.slint");

/// Every `.slint` under `components/onboarding/`, comment-stripped.
///
/// Its own walk rather than [`melodia_testkit::stripped_sources`] over the whole UI tree: the
/// question is about one directory, and a tree-wide floor would pass with the directory gone.
fn onboarding_sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(UI_DIR).join("components/onboarding");
    let mut unreadable = Vec::new();
    let mut out = Vec::new();

    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "slint") {
                    continue;
                }
                let name =
                    path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                match std::fs::read_to_string(&path) {
                    Ok(src) => out.push((name, strip_line_comments(&src))),
                    Err(e) => unreadable.push(format!("{}: {e}", path.display())),
                }
            }
        }
        Err(e) => unreadable.push(format!("{}: {e}", dir.display())),
    }

    // Collected rather than skipped: a path that won't read is indistinguishable from one holding
    // nothing, and this walk's whole value is that it sees every file.
    assert!(unreadable.is_empty(), "unreadable paths under components/onboarding: {unreadable:?}");
    assert!(
        out.len() >= MIN_ONBOARDING_SOURCES,
        "only {} .slint files under components/onboarding — the walk below would pass vacuously",
        out.len()
    );
    out
}

/// The card is mounted behind `if Onboarding.mounted`, and a `changed` handler inside a branch
/// that gets dropped outlives it: the generated `ChangeTracker` holds a `VWeakMapped` back to a
/// component nothing can upgrade to any more, and `evaluate` opens with an `unwrap`.
///
/// It only fires if something re-dirties the watched property *after* the drop — which is exactly
/// what the Settings ▸ About row does, by writing `Onboarding.open` to re-open the card. So this
/// builds, reviews clean, survives the first open, and panics on the second, naming a component
/// nowhere near whatever was edited.
///
/// The cure is the ban rather than care: Rust owns both the mount and unmount timers precisely so
/// nothing here needs a tracker, and the step indicator is hand-drawn dots rather than a `TabBar`
/// for the same reason.
#[test]
fn no_onboarding_component_carries_a_change_tracker() {
    let offenders: Vec<String> = onboarding_sources()
        .into_iter()
        .filter(|(_, src)| src.contains("changed "))
        .map(|(name, _)| name)
        .collect();

    assert!(
        offenders.is_empty(),
        "components/onboarding must carry no `changed` handler — a tracker in a dropped `if` \
         branch panics on the next open. Offenders: {offenders:?}"
    );
}

/// The card paints *under* `DialogOverlay`, and the Escape chain answers in the same order.
///
/// Step 2's folder picker raises a real `Dialog` when it fails, so a card mounted above the dialog
/// layer would cover the error it just caused and swallow the Escape meant to dismiss it. Both
/// halves are one edit away from disagreeing and neither fails a build.
#[test]
fn the_card_sits_under_the_dialog_in_paint_and_in_escape_order() {
    let sheet = strip_line_comments(APP_WINDOW);
    let paint = (sheet.find("if Onboarding.mounted:"), sheet.find("DialogOverlay {"));
    assert!(
        matches!(paint, (Some(card), Some(dialog)) if card < dialog),
        "the welcome card must be mounted before DialogOverlay, or a dialog raised from it paints \
         behind the card: {paint:?}"
    );

    let keys = strip_line_comments(SHORTCUT_SCOPE);
    let escape = (keys.find("if (Dialog.open) {"), keys.find("if (Onboarding.open) {"));
    assert!(
        matches!(escape, (Some(dialog), Some(card)) if dialog < card),
        "Escape must reach the dialog before the card, matching the paint order above: {escape:?}"
    );
}

/// Every non-Escape shortcut early-outs while the card is up, or Space pauses playback through the
/// backdrop. The card has no text input of its own, so nothing else swallows those keys.
#[test]
fn the_card_gates_the_non_escape_shortcuts() {
    let keys = strip_line_comments(SHORTCUT_SCOPE);
    assert!(
        keys.contains("if (Dialog.open || Onboarding.open) {"),
        "the non-Escape early-out must name the card as well as the dialog"
    );
}
