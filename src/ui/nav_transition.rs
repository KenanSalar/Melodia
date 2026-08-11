//! Tiny helpers around `Nav.pending-enter-from` — the global that drives
//! `melodia-ui/ui/components/view-transition.slint`'s enter direction. Centralising
//! the one-liner here keeps intent self-documenting at every call-site
//! and makes the contract easy to grep for.
//!
//! Caller contract (also documented at `Nav.pending-enter-from` in
//! `melodia-ui/ui/globals/nav.slint`): every helper must be invoked synchronously on
//! the UI thread, **before** the write to `Nav.selected-index` that flips a
//! content-panel `if` branch in `melodia-ui/ui/app-window.slint`. Setting it
//! after the flip is too late — the new `ViewTransition` will have already
//! sampled the stale value at mount.
//!
//! **The reach is a *page* mount and nothing smaller.** My Library's nine bodies
//! sit under a band that morphs its own height, so each of them takes a fixed
//! `below` and opts out of translating entirely while that morph is running
//! (`ViewTransition.slide`); none of them reads this global. So a `mark` that
//! isn't paired with a `selected-index` move — a same-tab drill, a same-tab back,
//! a same-section history step — writes a value nothing samples. Harmless, since
//! every path that does mount a page marks first, and kept at those sites because
//! the *cross-section* half of each is real.
//!
//! `below` (lateral sidebar nav) is written from Slint itself in
//! `melodia-ui/ui/layout/sidebar.slint`, so it has no Rust helper.

use slint::ComponentHandle;

use crate::{AppWindow, Nav, NavEnterFrom};

/// `left` — returning from a detail via its back button, for the close that also
/// restores the section the drill started in. A close that stays on My Library
/// mounts a body, not a page, and reads this not at all.
pub fn mark_drill_back(ui: &AppWindow) {
    ui.global::<Nav>().set_pending_enter_from(NavEnterFrom::Left);
}

/// Generic pass-through. Used by `open_*` futures where the direction is
/// a function parameter (so the caller decides between
/// [`NavEnterFrom::Right`] for a user drill-in and [`NavEnterFrom::Below`]
/// for a first-launch seed), and by `nav_history::replay`. Prefer
/// [`mark_drill_back`] at call-sites where the direction is fixed.
///
/// It is the drill that crosses a *section* this answers for — the one whose
/// `on_applied` hook flips `Nav.selected-index` in the same tick.
pub fn mark(ui: &AppWindow, enter_from: NavEnterFrom) {
    ui.global::<Nav>().set_pending_enter_from(enter_from);
}
