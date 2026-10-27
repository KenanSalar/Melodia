//! The recent searches' callbacks.

use std::sync::Arc;

use slint::ComponentHandle;

use crate::ui::search::{SearchUi, fetch::push_recent_rows_to_slint};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Search};

pub(super) fn wire(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // A pick fills the box and goes through the usual debounce, so its commit re-adds the term
    // to the history like any other.
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
                    Err(e) => log::warn!("search::remove_history: {}", describe(&e)),
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
                    Err(e) => log::warn!("search::clear_history: {}", describe(&e)),
                }
            });
        });
    }
}
