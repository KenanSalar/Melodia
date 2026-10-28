//! Where a settled keystroke in the page's single filter box lands, the box meaning something
//! different on each tab.
//!
//! The band hides the box over a station page, which has nothing to narrow, so [`sync_box`]
//! answers the page closing. On Browse the needle *is* the query, so a settled burst is a fresh
//! request and the debounce in front of it is load-bearing.

use std::sync::Arc;

use slint::ComponentHandle;

use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Radio};

use super::{RadioTab, RadioUi, browse, kept, mounted_tab, suggest};

pub fn dispatch(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, text: &str) {
    let g = ui.global::<Radio>();
    match mounted_tab(&g) {
        RadioTab::Browse => browse::set_query(ui, state, radio_ui, text),
        tab => kept::set_filter(ui, radio_ui, tab, text),
    }
    // After the dispatch, so the pills describe the needle now asked for; on every tab, `refresh`
    // deciding which tabs offer scopes.
    suggest::refresh(ui, radio_ui);
}

/// Puts the box back to whatever the tab under it is filtered by: a reseat, not a clear, a blanked
/// box leaving a query standing in Rust with nothing on screen naming it.
pub fn sync_box(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let mounted = match mounted_tab(&g) {
        RadioTab::Browse => browse::query_name(radio_ui),
        tab => kept::filter_text(radio_ui, tab),
    };
    // Outside the early return: the surface moved, so its pill row follows even where the needles
    // match.
    suggest::refresh(ui, radio_ui);
    if g.get_filter() == mounted.as_str() {
        return;
    }
    g.set_filter(mounted.into());
}
