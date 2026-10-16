//! Every right-click menu opens on the button's release, through one predicate.
//!
//! Slint closes an open popup on a press outside it and swallows that press, so a menu raised on
//! the press can't be moved by a right-click elsewhere: the press only closes the old menu, and the
//! user clicks twice. A menu raised on the release answers that same right-click with its own
//! release. A press-raised menu still opens on the first click and nothing about it looks wrong in
//! review, so the button itself is the needle: `ContextClick.raises-menu` is the one place that may
//! name it.

use melodia_testkit::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// The button every right-click menu answers to.
const RIGHT_BUTTON: &str = "PointerEventButton.right";

/// The predicate's file and how often it names the button. An exact count rather than a bare
/// exemption: a second spelling there is a second rule beside the predicate, and none at all
/// means the needle has gone stale and this walk reads nothing.
const SANCTIONED: [(&str, usize); 1] = [("components/context-menu.slint", 1)];

#[test]
fn only_the_shared_predicate_names_the_right_button() {
    let spelled: Vec<(String, usize)> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .map(|(path, src)| {
            let count = src.matches(RIGHT_BUTTON).count();
            (path, count)
        })
        .filter(|(_, count)| *count > 0)
        .collect();
    let sanctioned: Vec<(String, usize)> =
        SANCTIONED.iter().map(|(path, count)| ((*path).to_owned(), *count)).collect();

    assert_eq!(
        spelled, sanctioned,
        "a surface testing the right button itself opens its menu on the press, which a \
         right-click elsewhere can't move; ask `ContextClick.raises-menu` instead"
    );
}
