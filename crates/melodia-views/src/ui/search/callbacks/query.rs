//! The query box's callbacks: each keystroke, the debounced commit, and the compact/full Songs
//! toggle.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::ComponentHandle;

use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::list_selection;
use crate::ui::search::{SearchUi, apply, fetch};
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, Search};

pub(super) fn wire(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // Every keystroke starts compact and defers a pending history add. An emptied box is a soft
    // leave (`SearchUi::release_for_empty_query`), its selection dropped so no "{N} selected"
    // pill outlives the results.
    {
        let s = state.clone();
        let su = search_ui.clone();
        let weak = weak.clone();
        g.on_query_changed(move |text| {
            su.history_token.fetch_add(1, Ordering::Relaxed);
            if let Some(ui) = weak.upgrade() {
                ui.global::<Search>().set_show_all_tracks(false);
            }
            if text.trim().is_empty() {
                apply::clear_results_on_ui(&weak);
                if let Some(ui) = weak.upgrade() {
                    list_selection::clear_row_selection(&ui.global::<Search>());
                }
                let su = su.clone();
                s.runtime.spawn_blocking(move || su.release_for_empty_query());
            }
        });
    }
    {
        let s = state.clone();
        let su = search_ui.clone();
        let weak = weak.clone();
        g.on_commit_search(move |text| {
            let s = s.clone();
            let su = su.clone();
            let weak = weak.clone();
            let q = text.to_string();
            // Logged at the commit, behind the view's debounce or an Enter, not per keystroke.
            log::debug!("search: {q:?}");
            // A new query drops the selection, whose ids its results may not hold. Compared
            // against `last_query` rather than the live `Search.query`, which runs ahead of it.
            if q.trim() != su.state().last_query.lock().as_str()
                && let Some(ui) = weak.upgrade()
            {
                list_selection::clear_row_selection(&ui.global::<Search>());
            }
            spawn_logged!(s, "search::commit", fetch::kick_search(&s, &su, &weak, q));
        });
    }
    {
        let su = search_ui.clone();
        let weak = weak.clone();
        g.on_toggle_show_all_tracks(move || {
            if let Some(ui) = weak.upgrade() {
                let g = ui.global::<Search>();
                g.set_show_all_tracks(!g.get_show_all_tracks());
            }
            fetch::reapply_cached_results(&su, &weak);
        });
    }
}
