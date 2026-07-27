//! Tiny helpers around `Nav.pending-enter-from` — the global that drives
//! `melodia-ui/ui/components/view-transition.slint`'s enter direction. Centralising
//! the one-liner here keeps intent self-documenting at every call-site
//! and makes the contract easy to grep for.
//!
//! Caller contract (also documented at `Nav.pending-enter-from` in
//! `melodia-ui/ui/globals.slint`): every helper must be invoked synchronously on
//! the UI thread, **before** the property write that flips an `if`
//! branch in `melodia-ui/ui/app-window.slint` (`Nav.selected-index` or
//! `*Detail.*-id`). Setting it after the flip is too late — the new
//! `ViewTransition` will have already sampled the stale value at mount.
//!
//! `below` (lateral sidebar nav) is written from Slint itself in
//! `melodia-ui/ui/layout/sidebar.slint`, so it has no Rust helper.

use slint::ComponentHandle;

use crate::{AppWindow, Nav, NavEnterFrom};

/// `left` — returning from a detail via its back button.
pub fn mark_drill_back(ui: &AppWindow) {
    ui.global::<Nav>().set_pending_enter_from(NavEnterFrom::Left);
}

/// Generic pass-through. Used by `open_*` futures where the direction is
/// a function parameter (so the caller decides between
/// [`NavEnterFrom::Right`] for a user drill-in and [`NavEnterFrom::Below`]
/// for a first-launch seed). Prefer [`mark_drill_back`] at call-sites
/// where the direction is fixed.
pub fn mark(ui: &AppWindow, enter_from: NavEnterFrom) {
    ui.global::<Nav>().set_pending_enter_from(enter_from);
}
