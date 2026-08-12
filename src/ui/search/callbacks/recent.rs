//! `Search.*` recent-searches callbacks: pick (re-run a past query),
//! remove a single term, and clear the whole list. See
//! [`super::wire`].

use std::sync::Arc;

use slint::ComponentHandle;

use crate::library;
use crate::state::AppState;
use crate::ui::search::{SearchUi, fetch::push_recent_rows_to_slint};
use crate::{AppWindow, Search};

/// Wire the recent-pick / recent-remove / recent-clear callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // recent-pick: optimistically fill the SearchBar (so the user
    // sees the chip's text in the input immediately) and bump
    // `debounce-tick-pending` so the throttle Timer fires
    // `commit-search` after the usual 300 ms. The fresh
    // `kick_search` will re-add the term to history naturally.
    {
        let weak = weak.clone();
        g.on_recent_pick(move |term| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Search>();
            g.set_query(term);
            g.set_debounce_tick_pending(g.get_debounce_tick_pending() + 1);
        });
    }
    {
        let s = state.clone();
        let su = search_ui.clone();
        let weak = weak.clone();
        g.on_recent_remove(move |term| {
            let s = s.clone();
            let su = su.clone();
            let weak = weak.clone();
            let term = term.to_string();
            s.runtime.clone().spawn(async move {
                match library::search::remove_search_history(&s, term).await {
                    Ok(rows) => {
                        (*su.state().recent.lock()).clone_from(&rows);
                        push_recent_rows_to_slint(&weak, rows);
                    }
                    Err(e) => log::warn!("search::remove_history: {e}"),
                }
            });
        });
    }
    {
        let s = state.clone();
        let su = search_ui.clone();
        let weak = weak.clone();
        g.on_recent_clear(move || {
            let s = s.clone();
            let su = su.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                match library::search::clear_search_history(&s).await {
                    Ok(rows) => {
                        (*su.state().recent.lock()).clone_from(&rows);
                        push_recent_rows_to_slint(&weak, rows);
                    }
                    Err(e) => log::warn!("search::clear_history: {e}"),
                }
            });
        });
    }
}
