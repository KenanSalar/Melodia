//! Every popup closes when the OS window loses focus.
//!
//! Slint closes a `PopupWindow` on a click outside it and never on a focus change, so a popup with
//! nothing watching stays open behind another application. Its trigger, lit off the popup's
//! `is-open`, goes on looking pressed with nothing under it, and the next click lands on a menu the
//! user had stopped looking at.
//!
//! The guard has two spellings and both count: a `FocusLossWatcher` mounted inside the popup, or a
//! `MenuSurface`, which mounts one for its host. Each popup's own braces are read, since a watcher
//! elsewhere in its file is live while the popup is shut and guards nothing. A popup taking
//! neither is unguarded, and nothing about it looks wrong.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, blocks_named, stripped_sources};

/// A popup's element name, whose block is what gets read.
const POPUP: &str = "PopupWindow";

/// The watcher itself, mounted by the popup.
const WATCHER: &str = "FocusLossWatcher {";

/// The surface that mounts one on its host's behalf.
const SURFACE: &str = "MenuSurface {";

/// Vacuity floor on the popups found. Loose enough that retiring one does not trip it, and far
/// enough above zero that a walk whose needle has gone stale cannot pass reading nothing.
const MIN_POPUPS: usize = 10;

#[test]
fn every_popup_dismisses_on_focus_loss() {
    let mut unguarded = Vec::new();
    let mut found = 0usize;

    for (path, source) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        for popup in blocks_named(&source, POPUP) {
            found += 1;
            if !popup.contains(WATCHER) && !popup.contains(SURFACE) {
                unguarded.push(path.clone());
            }
        }
    }

    assert!(
        unguarded.is_empty(),
        "{unguarded:?} declare a popup that stays open when the window loses focus — mount a \
         `FocusLossWatcher` inside it, or put its rows in a `MenuSurface`"
    );
    assert!(
        found >= MIN_POPUPS,
        "only {found} popups found, so this walk has stopped reading the thing it pins"
    );
}
