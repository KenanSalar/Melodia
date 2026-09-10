//! Every singleton popup closes when the OS window loses focus.
//!
//! Slint 1.16 gives a `PopupWindow` no `closed` callback, so a popup that only closes on a click
//! outside stays open behind another application and leaves `PopupHighlight.id` set. That id is
//! what lights the trigger, so the control it belongs to goes on looking pressed with nothing
//! under it, and the next click lands on a menu the user had stopped looking at.
//!
//! The guard has two spellings and both count: a `FocusLossWatcher` mounted behind the file's own
//! `PopupHighlight.id` gate, or a `MenuSurface` handed the same id, which mounts one for its host.
//! A file taking neither is unguarded, and nothing about it looks wrong.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// The write that makes a popup a singleton, and so the thing that identifies one here.
const CLAIM: &str = "PopupHighlight.id = \"";

/// The watcher itself, mounted by the popup.
const WATCHER: &str = "FocusLossWatcher";

/// The surface that mounts one on its host's behalf, keyed by the same id.
const SURFACE_KEY: &str = "popup-id: \"";

/// Vacuity floor on the popups found. Loose enough that retiring one does not trip it, and far
/// enough above zero that a walk whose needle has gone stale cannot pass reading nothing.
const MIN_SINGLETON_POPUPS: usize = 6;

/// The non-empty ids a file claims. The empty one is every close path handing the id back, which
/// names no popup and would match every file that clears it.
fn claimed_ids(source: &str) -> Vec<String> {
    source
        .match_indices(CLAIM)
        .filter_map(|(at, _)| {
            let rest = &source[at + CLAIM.len()..];
            let id = rest.split('"').next()?;
            (!id.is_empty()).then(|| id.to_owned())
        })
        .collect()
}

#[test]
fn every_popup_that_claims_the_highlight_dismisses_on_focus_loss() {
    let mut unguarded = Vec::new();
    let mut found = 0usize;

    for (path, source) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        let ids = claimed_ids(&source);
        if ids.is_empty() {
            continue;
        }
        found += 1;

        let watches = source.contains(WATCHER);
        let delegates = ids.iter().any(|id| source.contains(&format!("{SURFACE_KEY}{id}\"")));
        if !watches && !delegates {
            unguarded.push(format!("{path}: claims {ids:?}"));
        }
    }

    assert!(
        unguarded.is_empty(),
        "{unguarded:?} claim the popup highlight and never hand it back on focus loss — mount a \
         `FocusLossWatcher` behind the id gate, or pass the id to a `MenuSurface`"
    );
    assert!(
        found >= MIN_SINGLETON_POPUPS,
        "only {found} singleton popups found, so this walk has stopped reading the thing it pins"
    );
}

/// The delegating half is only a guard while the id it forwards is the one the trigger wrote.
/// `MenuSurface` defaults `popup-id` to `""` and refuses to arm on it for exactly this reason, so
/// a mismatch is a live watcher on nothing rather than a build failure.
#[test]
fn a_popup_that_delegates_its_guard_forwards_the_id_it_claims() {
    let mut mismatched = Vec::new();
    let mut delegating = 0usize;

    for (path, source) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        if !source.contains(SURFACE_KEY) {
            continue;
        }
        delegating += 1;
        for id in claimed_ids(&source) {
            if !source.contains(&format!("{SURFACE_KEY}{id}\"")) {
                mismatched.push(format!("{path}: claims {id:?}"));
            }
        }
    }

    assert!(
        mismatched.is_empty(),
        "{mismatched:?} hand a `MenuSurface` an id other than the one they claim, so the watcher \
         it mounts can never fire"
    );
    assert!(
        delegating > 0,
        "no popup delegates its guard any more, so this walk reads nothing — retire it, or fix \
         the needle it has stopped finding"
    );
}
