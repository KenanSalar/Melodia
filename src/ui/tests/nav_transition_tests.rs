//! Source pin for the enter-direction convention: **every Slint-side mount writes
//! its own edge before it mounts.**
//!
//! `Nav.pending-enter-from` is global and enter-only, so an unwritten edge is not a
//! default — it is whatever the last drill or back left in it. `mark_drill_back`
//! fires on every detail close including the same-page ones that consume nothing,
//! so the value sitting there between navigations is reliably `left`, and a mount
//! that forgets to mark slides in sideways for no reason anyone can see in the file
//! it happens in.
//!
//! Three sites had it and three didn't, all six spelled the same way and none of
//! them wrong on its own page: Ctrl+, and Browse's "Open Settings" pill flip the
//! nav index, and closing Now Playing — five entry points — destroys that branch and
//! remounts the view underneath, which is a fresh `ViewTransition` either way. So
//! back out of an album, open Now Playing, close it, and the page arrived from the
//! left.
//!
//! **A walk rather than a list**, the `file_dialog` reason: the site that gets this
//! wrong is the one nobody has written yet. `below` specifically, not any direction
//! — a Slint-side mount is lateral by construction, since the two that aren't (a
//! drill in, a back out) are Rust's and go through `ui::nav_transition`.

use crate::test_support::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// The nine writes as of this pin — six closers (five Now Playing entry points plus
/// the sidebar's, which dismisses it on any nav click) and three index flips. A
/// floor rather than an equality: a tenth is fine, a walk that stopped finding them
/// is not.
const MIN_MOUNTS: usize = 9;

const MARK: &str = "Nav.pending-enter-from = NavEnterFrom.below;";

/// How far back the mark may sit. One line covers seven of the nine; the sidebar
/// puts two writes under one mark, and the miniplayer swap owes its mark to the
/// swap itself rather than to the close nested in the `if` below it.
const LOOKBACK: usize = 3;

/// Does this line mount something that samples `Nav.pending-enter-from`?
///
/// `==` is excluded at both properties — every branch selector in
/// `app-window.slint` reads them — and so is a write of `true` to
/// `now-playing-open`, which mounts the Now Playing branch and that one hard-codes
/// its own `below`. Nothing spells that today; it is here so the predicate says
/// which half of the toggle it means.
fn mounts_a_view(line: &str) -> bool {
    let flips_index =
        line.starts_with("Nav.selected-index =") && !line.starts_with("Nav.selected-index ==");
    let closes_now_playing = line.starts_with("Nav.now-playing-open =")
        && !line.starts_with("Nav.now-playing-open ==")
        && !line.starts_with("Nav.now-playing-open = true");
    flips_index || closes_now_playing
}

#[test]
fn every_slint_side_mount_writes_its_own_enter_edge() {
    let mut found = 0usize;

    for (path, src) in stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES) {
        // Comment-stripped leaves the line structure with blank tails, so the
        // lookback counts *code* lines — otherwise a long comment block above a
        // write pushes its own mark out of the window.
        let code: Vec<&str> = src.lines().map(str::trim).filter(|line| !line.is_empty()).collect();

        for (i, line) in code.iter().enumerate() {
            if !mounts_a_view(line) {
                continue;
            }
            found += 1;
            let marked = code[i.saturating_sub(LOOKBACK)..i].contains(&MARK);
            assert!(
                marked,
                "{path}: `{line}` mounts a view without writing its enter edge first — \
                 `{MARK}` must sit within {LOOKBACK} lines above it. Unwritten, the new \
                 mount samples whatever the last drill or back left in the global, which \
                 is reliably `left`"
            );
        }
    }

    assert!(
        found >= MIN_MOUNTS,
        "only {found} Slint-side view mounts found, expected at least {MIN_MOUNTS} — the \
         walk stopped seeing them, so every assertion above it is vacuous"
    );
}
