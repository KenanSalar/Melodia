//! The fetch behind a committed query, and the delayed history add.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use slint::{ComponentHandle, SharedString, Weak};

use super::SearchUi;
use super::apply::{apply_results_to_slint, clear_results_on_ui, set_loading_on_ui};
use crate::ui::model_diff::replace_rows;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::search::SearchResults;
use melodia_core::error::{AppResult, describe};
use melodia_ui::{AppWindow, Search};

/// Runs `search_all` and applies it unless a newer commit moved the fetch token meanwhile.
/// `loading` goes up before the await and down with the apply; an empty query clears instead.
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

    let results = library::search::search_all(&state.db, trimmed.clone()).await?;

    let token_now = search_ui.fetch_token.load(Ordering::Relaxed);
    if token_now != my_token {
        // Superseded. `loading` stays up, being the newer fetch's to lower.
        return Ok(());
    }

    if prewarm_result_covers(search_ui, &results).await {
        // The prewarm yielded, so a commit landing during it wins.
        if search_ui.fetch_token.load(Ordering::Relaxed) != my_token {
            return Ok(());
        }
    }

    *search_ui.state().last_results.lock() = Some(results.clone());
    (*search_ui.state().last_query.lock()).clone_from(&trimmed);
    apply_results_to_slint(search_ui, weak, &results, &trimmed);
    set_loading_on_ui(weak, false);

    schedule_history_add(state, search_ui, weak, query);
    Ok(())
}

/// Warms every result surface's covers, each against its own tier, since a miss decodes on the
/// UI thread at paint time. Returns whether it yielded, which obliges the caller to re-check its
/// token.
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

/// Re-derives everything visible from `last_results` without a query, for the "Show all" toggle
/// and a re-sort. A no-op before the first commit.
pub fn reapply_cached_results(search_ui: &Arc<SearchUi>, weak: &Weak<AppWindow>) {
    let Some(results) = search_ui.state().last_results.lock().clone() else {
        return;
    };
    let query = search_ui.state().last_query.lock().clone();
    apply_results_to_slint(search_ui, weak, &results, &query);
}

/// Adds `query` to the history after a pause, unless a keystroke moves the history token first:
/// a query the user paused on is worth remembering, a passing typo isn't.
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
            return;
        }
        match library::search::add_search_history(&s, query).await {
            Ok(rows) => push_recent_rows_to_slint(&weak, rows),
            Err(e) => log::warn!("search::add_history: {}", describe(&e)),
        }
    });
}

pub fn push_recent_rows_to_slint(weak: &Weak<AppWindow>, rows: Vec<String>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let rows = rows.into_iter().map(SharedString::from).collect();
        replace_rows(&ui.global::<Search>().get_recent_rows(), rows, "Search.recent-rows");
    });
}
