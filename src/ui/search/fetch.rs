//! Search fetch flow: debounced commit → FTS+LIKE round-trip → stale-token
//! guard → cover prewarm → apply. Plus the 2-second delayed history-add
//! scheduler.
//!
//! Where the results *go* is [`super::apply`]; what wins the Top Result card
//! is [`super::top_result`].

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::SearchUi;
use super::apply::{apply_results_to_slint, clear_results_on_ui, set_loading_on_ui};
use crate::error::AppResult;
use crate::library;
use crate::library::search::SearchResults;
use crate::state::AppState;
use crate::{AppWindow, Search};

/// Bump the fetch token, run `search_all`, and (if the token is still
/// current after the SQL resolves) apply the result to the Slint
/// models. A newer keystroke that supersedes this one will have moved
/// the token, and the post-fetch check drops the stale UI write.
///
/// Loading state: set ON synchronously *before* the await so the view's
/// "Searching…" line is visible during the round-trip; turned OFF in
/// the apply step. Empty query bypasses the fetch and clears
/// everything synchronously.
pub async fn kick_search(
    state: &AppState,
    search_ui: &Arc<SearchUi>,
    weak: &Weak<AppWindow>,
    query: String,
) -> AppResult<()> {
    let my_token = search_ui.fetch_token.fetch_add(1, Ordering::Relaxed) + 1;
    let trimmed = query.trim().to_owned();
    if trimmed.is_empty() {
        clear_results_on_ui(weak);
        return Ok(());
    }
    set_loading_on_ui(weak, true);

    let results = library::search::search_all(state, trimmed.clone()).await?;

    let token_now = search_ui.fetch_token.load(Ordering::Relaxed);
    if token_now != my_token {
        // Newer keystroke superseded us. Don't paint the stale set —
        // the in-flight kick_search for the newer query will. Also
        // don't reset `loading`: that in-flight task is between its
        // `set_loading_on_ui(true)` and apply step, and toggling here
        // would race.
        return Ok(());
    }

    if prewarm_result_covers(search_ui, &results).await {
        // The decode burst yielded — re-check staleness so a query that
        // arrived mid-prewarm wins the paint (same no-`loading`-reset
        // contract as the post-fetch check above).
        if search_ui.fetch_token.load(Ordering::Relaxed) != my_token {
            return Ok(());
        }
    }

    *search_ui.state().last_results.lock() = Some(results.clone());
    (*search_ui.state().last_query.lock()).clone_from(&trimmed);
    apply_results_to_slint(search_ui, weak, &results, &trimmed);
    set_loading_on_ui(weak, false);

    // Schedule the 2-second delayed history add. The 2 s window is
    // Tauri parity — a query the user paused on long enough is worth
    // remembering; a fast typo isn't.
    schedule_history_add(state, search_ui, weak, query);
    Ok(())
}

/// Warm every result surface's covers off-thread before the apply. Returns
/// whether it actually yielded, which is what obliges the caller to re-check
/// its fetch token.
///
/// Track rows finish on the UI thread (`finish_track_list_row`) and the Album
/// / Artist strip cards resolve via lazy `request-*-cover` lookups that decode
/// on miss *on the UI thread* — without this, a cold cache pays one
/// synchronous decode per card at paint time.
///
/// Three prewarms rather than one because the three lists land in three
/// different tiers, so each is capped against its own capacity. Result sets
/// are LIMIT-bounded, so no cap binds today.
async fn prewarm_result_covers(search_ui: &Arc<SearchUi>, results: &SearchResults) -> bool {
    let track_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        results.tracks.iter().map(|t| t.artwork_path.as_deref()),
        search_ui.cover_thumbs.capacity(),
    );
    let album_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        results.albums.iter().map(|a| a.artwork_path.as_deref()),
        search_ui.album_strip_thumbs.capacity(),
    );
    let artist_covers: Vec<PathBuf> = crate::ui::grid_prewarm::unique_artwork_paths(
        results.artists.iter().map(|a| a.image_path.as_deref()),
        search_ui.artist_strip_thumbs.capacity(),
    );
    if track_covers.is_empty() && album_covers.is_empty() && artist_covers.is_empty() {
        return false;
    }

    let row_thumbs = search_ui.cover_thumbs.clone();
    let album_thumbs = search_ui.album_strip_thumbs.clone();
    let artist_thumbs = search_ui.artist_strip_thumbs.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if !track_covers.is_empty() {
            row_thumbs.prewarm(&track_covers);
        }
        if !album_covers.is_empty() {
            album_thumbs.prewarm(&album_covers);
        }
        if !artist_covers.is_empty() {
            artist_thumbs.prewarm(&artist_covers);
        }
    })
    .await;
    true
}

/// Cached-results swap: re-derive the visible Songs slice from
/// `last_results` according to the *current* `show-all-tracks` flag
/// and active sort. Called from the `toggle-show-all-tracks` callback
/// — no DB hit; the apply step owns the slice/sort logic.
pub fn swap_tracks_compact_or_full(search_ui: &Arc<SearchUi>, weak: &Weak<AppWindow>) {
    let Some(results) = search_ui.state().last_results.lock().clone() else {
        return;
    };
    let query = search_ui.state().last_query.lock().clone();
    apply_results_to_slint(search_ui, weak, &results, &query);
}

/// 2-second delayed history add. The history token is bumped on every
/// keystroke (via the `query-changed` callback in `callbacks::query`); the
/// delayed task captures the token at spawn time and bails if a newer
/// keystroke moved it. After persisting, pushes the returned ordered
/// `Vec<String>` into `Search.recent-rows` so the empty-input state
/// surfaces the latest add at the top.
pub fn schedule_history_add(
    state: &AppState,
    search_ui: &Arc<SearchUi>,
    weak: &Weak<AppWindow>,
    query: String,
) {
    let my_token = search_ui.history_token.load(Ordering::Relaxed);
    let s = state.clone();
    let su = search_ui.clone();
    let weak = weak.clone();
    state.runtime.clone().spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if su.history_token.load(Ordering::Relaxed) != my_token {
            return; // newer keystroke since we scheduled; bail.
        }
        match library::search::add_search_history(&s, query).await {
            Ok(rows) => {
                (*su.state().recent.lock()).clone_from(&rows);
                push_recent_rows_to_slint(&weak, rows);
            }
            Err(e) => log::warn!("search::add_history: {e}"),
        }
    });
}

/// Push a freshly-loaded recent-searches list into `Search.recent-rows`.
/// Called from `callbacks::recent` on initial hydrate, after a successful
/// history-add, and after `recent-remove` / `recent-clear`.
pub fn push_recent_rows_to_slint(weak: &Weak<AppWindow>, rows: Vec<String>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Search>();
        let model = g.get_recent_rows();
        let Some(vm) = model.as_any().downcast_ref::<VecModel<SharedString>>() else {
            log::warn!("Search.recent-rows: VecModel<SharedString> downcast failed");
            return;
        };
        let ss: Vec<SharedString> = rows.into_iter().map(SharedString::from).collect();
        vm.set_vec(ss);
    });
}
