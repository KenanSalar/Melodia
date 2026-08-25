//! Where a settled keystroke in the page's single filter box lands.
//!
//! A function from the outset rather than an `if` that grows, for the reason My Library's
//! `dispatch` is one: the box means something different on each tab, and the station page over
//! them is a fourth destination.
//!
//! **What Browse does with it is not what the other two do.** Every other filter box in the tree
//! narrows rows already in hand; there the needle *is* the query, so a settled burst is a fresh
//! request and the debounce in front of it is load-bearing rather than an optimization.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::{AppWindow, Radio};

use super::{RadioTab, RadioUi, browse, detail, kept, tab_from_index};

pub fn dispatch(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, text: &str) {
    let g = ui.global::<Radio>();
    // The detail is tested first because it is the surface *over* a tab rather than one of them:
    // with a station page open the tab under it is not what the box narrows.
    if g.get_detail_open() {
        detail::set_filter(ui, radio_ui, text);
        return;
    }
    match tab_from_index(&g, g.get_tab_idx()) {
        RadioTab::Browse => browse::set_query(ui, state, radio_ui, text),
        tab => kept::set_filter(ui, radio_ui, tab, text),
    }
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
    let mounted = if g.get_detail_open() {
        detail::filter_text(radio_ui)
    } else {
        match tab_from_index(&g, g.get_tab_idx()) {
            RadioTab::Browse => browse::query_name(radio_ui),
            tab => kept::filter_text(radio_ui, tab),
        }
    };
    if g.get_filter() == mounted.as_str() {
        return;
    }
    g.set_filter(mounted.into());
}
