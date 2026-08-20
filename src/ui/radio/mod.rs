//! The Radio page — two tabs (Browse, Favorites) over the radio-browser.info directory and the
//! stations the user kept.
//!
//! **One section handle for both tabs**, and one `SectionActiveGate` to match. My Library's
//! per-tab gates are what its history left it — five tabs that used to be five sidebar sections,
//! each with a hook of its own — where here a tab flip has to stay inside the page: Browse holds a
//! directory answer bought with a network round trip, and a per-tab gate would hand it back every
//! time the user glanced at their favorites.

mod callbacks;
mod tabs;

use std::sync::Arc;

use crate::ui::section_state::SectionState;
use crate::ui::view_ctx::ViewCtx;

use tabs::section_is_up;
pub use tabs::{RadioTab, seed_tab, tab_from_index};

/// This page's `Nav.selected-index`, and the single definition of it.
pub const NAV_RADIO: i32 = 10;

/// Wire every `Radio.*` callback.
///
/// Returns nothing: each wired closure clones its own strong `Arc`, so the handle built here is
/// kept alive by the wiring rather than by a caller holding it.
pub fn install(cx: ViewCtx<'_>) {
    let radio_ui = Arc::new(RadioUi::new(section_is_up(cx.app)));
    callbacks::wire(cx.app, cx.state, &radio_ui);
}

/// Rust-side state for the Radio page.
pub struct RadioUi {
    /// Whether the page is on screen, and whether what it cached went stale while it wasn't.
    /// **Seeded at wire time rather than left to the gate**, which fires on transitions only and
    /// whose `ChangeTracker` baselines silently inside `AppWindow::new()` — a section seeded
    /// wrong has no edge left to correct it.
    pub section: SectionState,
}

impl RadioUi {
    fn new(section_active: bool) -> Self {
        let section = SectionState::new();
        section.set_active(section_active);
        Self { section }
    }
}
