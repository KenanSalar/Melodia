//! Helpers around `Nav.pending-enter-from`, the global driving
//! `melodia-ui/ui/components/view-transition.slint`'s enter direction.
//!
//! Write it synchronously on the UI thread **before** the `Nav.selected-index` move that
//! flips a content-panel `if` — a `ViewTransition` samples it at mount, so a write after
//! the flip is one navigation late.
//!
//! **The reach is a *page* mount and nothing smaller.** My Library's nine bodies read this
//! not at all, so a `mark` unpaired with a `selected-index` move writes a value nothing
//! samples; kept at those sites for the cross-section half.

use slint::ComponentHandle;

use crate::{AppWindow, Nav, NavEnterFrom};

/// `left` — a detail close that also restores the section the drill started in.
pub fn mark_drill_back(ui: &AppWindow) {
    ui.global::<Nav>().set_pending_enter_from(NavEnterFrom::Left);
}

/// Generic pass-through, for `open_*` futures taking the direction as a parameter and for
/// `nav_history::replay`. Prefer [`mark_drill_back`] where the direction is fixed.
pub fn mark(ui: &AppWindow, enter_from: NavEnterFrom) {
    ui.global::<Nav>().set_pending_enter_from(enter_from);
}
