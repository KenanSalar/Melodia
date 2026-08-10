//! `Search.*` query callbacks: keystroke debounce, commit → FTS fetch,
//! and the compact/full Songs toggle. See [`super::wire`].

use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::ComponentHandle;

use crate::state::AppState;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::search::{self as search_ui_mod, SearchUi, apply, fetch};
use crate::{AppWindow, Search};

/// Wire the query / debounce / show-all callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, search_ui: &Arc<SearchUi>) {
    let g = ui.global::<Search>();
    let weak = ui.as_weak();

    // Every keystroke: mirror the trimmed text into Rust state, reset
    // `show-all-tracks` (Tauri parity: every new query starts compact),
    // bump the history token (defers any pending 2-second history-add
    // for a previous query), and fast-path the empty case so the
    // recent-searches branch re-mounts as soon as the user clears
    // the SearchBar.
    //
    // Empty-input also releases the strip-tier LRUs + cached results
    // off-thread: the user explicitly cleared the field, so we treat
    // it like a soft section-leave (mirrors `release_section_state`'s
    // ordering but without the section-active bail). UI-side
    // selection is dropped synchronously so the leftover
    // "{N} selected" pill from the previous query doesn't linger.
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
                    search_ui_mod::clear_selection(&ui, &su);
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
            // Reset selection if this commit targets a different
            // query than the one whose results currently populate
            // the model. Previous selection ids reference tracks
            // that may not appear in the new result set, so the
            // "{N} selected" pill would lie. Compare against the
            // cached `last_query` (not `Search.query`, which is
            // the live keystroke text — they're equal at commit
            // time only when the throttle Timer just fired against
            // a quiet field).
            if q.trim() != su.state().last_query.lock().as_str()
                && let Some(ui) = weak.upgrade()
            {
                search_ui_mod::clear_selection(&ui, &su);
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
            fetch::swap_tracks_compact_or_full(&su, &weak);
        });
    }
}
