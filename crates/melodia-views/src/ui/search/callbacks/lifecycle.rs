//! The Search section's leave, the models on the UI thread and then the tiers off it, and its
//! return, which re-runs the query the leave kept.

use std::sync::Arc;

use slint::ComponentHandle;

use super::NAV_SEARCH;
use crate::ui::search::{SearchUi, apply::teardown_models_on_leave};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Nav, Search};

pub(super) fn wire(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // Seeded here, the gate firing only on a later difference.
    search_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_SEARCH);
    {
        let su = search_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            su.set_section_active(active);
            if active {
                if let Some(ui) = weak.upgrade() {
                    rerun_kept_query(&ui);
                }
                return;
            }
            if let Some(ui) = weak.upgrade() {
                teardown_models_on_leave(&ui);
            }
            let su = su.clone();
            s.runtime.spawn_blocking(move || su.release_section_state());
        });
    }
}

/// Commits the query a leave kept, as Enter does, the leave having released its results: left
/// alone, the box would hold the query over "No results found".
fn rerun_kept_query(ui: &AppWindow) {
    let g = ui.global::<Search>();
    let query = g.get_query();
    if query.trim().is_empty() {
        return;
    }
    // Raised now rather than when the fetch posts it, which would let the empty state paint first.
    g.set_loading(true);
    // A keystroke still pending at the leave would otherwise commit a second time.
    g.set_debounce_tick_applied(g.get_debounce_tick_pending());
    g.invoke_commit_search(query);
}
