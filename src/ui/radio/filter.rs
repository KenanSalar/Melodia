//! Where a settled keystroke in the page's single filter box lands.
//!
//! A function rather than an `if`, for the reason My Library's `dispatch` is one: the box means
//! something different on each tab.
//!
//! **The station page is not one of the destinations, and the box is not shown over it.** A
//! station has no songs of its own to narrow — its page is a list of facts about a stream — so
//! the band hides the box while one is open rather than offering a control with nothing to do.
//! What [`sync_box`] then answers is the page *closing*, and the tab's needle coming back.
//!
//! **What Browse does with it is not what the other two do.** Every other filter box in the tree
//! narrows rows already in hand; there the needle *is* the query, so a settled burst is a fresh
//! request and the debounce in front of it is load-bearing rather than an optimization.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::{AppWindow, Radio};

use super::{RadioTab, RadioUi, browse, kept, mounted_tab, suggest};

pub fn dispatch(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, text: &str) {
    let g = ui.global::<Radio>();
    match mounted_tab(&g) {
        RadioTab::Browse => browse::set_query(ui, state, radio_ui, text),
        tab => kept::set_filter(ui, radio_ui, tab, text),
    }
    // After the dispatch, so the pills describe the needle Browse is now asking for rather than
    // the one it was. Called on every tab, `refresh` being the one place that decides a tab has no
    // scopes to offer.
    suggest::refresh(ui, radio_ui);
}

/// Put the box back to whatever the tab under it is filtered by.
///
/// **A reseat, not a clear**, which is the opposite of what a tab pick does on My Library and for
/// the opposite reason. There the entering tab's needle is dropped because both surfaces are
/// grids that would silently hide rows; here the *leaving* tab's needle is a query living in Rust
/// that a blanked box would leave standing with nothing on screen naming it. So the needle stays
/// with its tab and the box follows the mount.
///
/// The box is given up either way: it now says something the user did not type on this tab.
pub fn sync_box(ui: &AppWindow, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let mounted = match mounted_tab(&g) {
        RadioTab::Browse => browse::query_name(radio_ui),
        tab => kept::filter_text(radio_ui, tab),
    };
    // Outside the early return below: what moved is the *surface* under the box, and a tab whose
    // needle happens to match the one being left still has to gain or lose its pill row.
    suggest::refresh(ui, radio_ui);
    if g.get_filter() == mounted.as_str() {
        return;
    }
    g.set_filter(mounted.into());
}
