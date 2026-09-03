//! Search section lifecycle: the `section-active-changed` enter/leave
//! handler — UI-thread model teardown + off-thread cache release. See
//! [`super::wire`].

use std::sync::Arc;

use slint::ComponentHandle;

use super::NAV_SEARCH;
use crate::state::AppState;
use crate::ui::search::{SearchUi, apply::teardown_models_on_leave};
use crate::{AppWindow, Nav, Search};

/// Wire the Search section-lifecycle callback.
pub(super) fn wire(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // Seed the synchronous shadow from the current nav state: the gate's
    // `ChangeTracker` baselines inside `AppWindow::new()` and fires only on a
    // later difference, so a section the boot doesn't land on gets no edge at
    // all, and the one it does land on gets its edge a frame late — after
    // boot has already read this shadow. See the `SectionActiveGate` bullet
    // in `.claude/rules/ui-patterns.md`.
    search_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_SEARCH);
    {
        let su = search_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            su.set_section_active(active);
            if !active && let Some(ui) = weak.upgrade() {
                // UI-thread teardown — clear the result models +
                // Top Result scalars so the LRU drop pairs with
                // already-released Slint properties (no lingering
                // SharedPixelBuffer Arcs). Keeps `query` +
                // `recent-rows` so a brief flip away and back lands
                // on the same view state.
                teardown_models_on_leave(&ui);
            }
            if !active {
                // Off-thread heavy release — runs after the UI
                // teardown so the LRU clear pairs with already-
                // released Slint properties.
                let su = su.clone();
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || su.release_section_state());
            }
        });
    }
}
