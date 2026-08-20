//! Where a settled keystroke in the page's single filter box lands.
//!
//! A function from the outset rather than an `if` that grows, for the reason My Library's
//! `dispatch` is one: the box means something different on each tab, and Phase 8's station detail
//! adds a third destination. Today Browse is the only tab with anything to filter, so this is a
//! guard clause rather than a match with an empty arm.
//!
//! **What Browse does with it is not what any other surface does.** Every other filter box in the
//! tree narrows rows already in hand; here the needle *is* the query, so a settled burst is a
//! fresh request and the debounce in front of it is load-bearing rather than an optimization.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::{AppWindow, Radio};

use super::{RadioTab, RadioUi, browse, tab_from_index};

pub fn dispatch(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, text: &str) {
    let g = ui.global::<Radio>();
    if tab_from_index(&g, g.get_tab_idx()) != RadioTab::Browse {
        return;
    }
    browse::set_query(ui, state, radio_ui, text);
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
    let mounted = match tab_from_index(&g, g.get_tab_idx()) {
        RadioTab::Browse => browse::query_name(radio_ui),
        // Phase 7 gives the kept list a needle of its own; until then it filters nothing and the
        // box is empty on it.
        RadioTab::Favorites => String::new(),
    };
    if g.get_filter() == mounted.as_str() {
        return;
    }
    g.set_filter(mounted.into());
}
