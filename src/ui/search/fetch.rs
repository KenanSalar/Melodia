//! Search fetch flow: debounced commit → FTS+LIKE round-trip →
//! stale-token guard → Slint-model apply. Plus the pure-function Top
//! Result ranking (unit-tested) and the 2-second delayed history-add
//! scheduler.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use slint::{ComponentHandle, Image, Model, SharedString, VecModel, Weak};

use super::state::COMPACT_TRACK_LIMIT;
use super::{
    SearchUi, restamp_rows, to_slint_album_strip_row, to_slint_artist_strip_row,
};
use crate::database::queries::SearchResults;
use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::error::AppResult;
use crate::library;
use crate::services::settings::SortDir;
use crate::state::AppState;
use crate::ui::tracks::{PreparedTrackRow, finish_track_list_row};
use crate::ui::track_sort::sort_track_rows_by;
use crate::{
    AppWindow, EntityStripRow as UiEntityStripRow, Search, TrackListRow as UiTrackListRow,
};

/// Top Result discriminator. Matches the `top-kind` string slot in the
/// Slint `Search` global ("album" / "artist" / "").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopKind {
    Album,
    Artist,
}

/// Top Result payload — the scalar fields the Slint `Search` global
/// holds (no struct on the Slint side; the discriminator + scalars
/// avoid baking a `kind` field into `EntityStripRow`).
#[derive(Debug, Clone)]
pub struct TopResult {
    pub kind: TopKind,
    pub id: i64,
    pub title: String,
    pub subtitle: String,
    pub artwork_path: Option<String>,
}

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

    // Prewarm every result surface's covers off-thread before the apply:
    // track rows finish on the UI thread (`finish_track_list_row`) and
    // the Album / Artist strip cards resolve via lazy `request-*-cover`
    // lookups that decode on miss *on the UI thread* — without this, a
    // cold cache pays one synchronous decode per card at paint time.
    // Result sets are LIMIT-bounded so the prewarm set is small;
    // `prewarm` dedupes its input.
    let track_covers: Vec<PathBuf> = results
        .tracks
        .iter()
        .filter_map(|t| t.artwork_path.as_deref().map(PathBuf::from))
        .collect();
    let album_covers: Vec<PathBuf> = results
        .albums
        .iter()
        .filter_map(|a| a.artwork_path.as_deref().map(PathBuf::from))
        .collect();
    let artist_covers: Vec<PathBuf> = results
        .artists
        .iter()
        .filter_map(|a| a.image_path.as_deref().map(PathBuf::from))
        .collect();
    if !(track_covers.is_empty() && album_covers.is_empty() && artist_covers.is_empty()) {
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

/// Push the freshly-fetched `results` into all three Slint models,
/// compute + apply the Top Result, set `tracks-total`, and honour the
/// `show-all-tracks` toggle (the apply path always re-derives the
/// visible Songs slice — that way callers don't have to remember to
/// reset `show-all-tracks` themselves).
pub fn apply_results_to_slint(
    search_ui: &Arc<SearchUi>,
    weak: &Weak<AppWindow>,
    results: &SearchResults,
    query: &str,
) {
    let sort = search_ui.state().sort.lock().clone();

    // Apply the in-memory sort to a copy of the tracks (the FTS5
    // `ORDER BY rank` lands them rank-ordered, but the user may have
    // picked a different sort after the fact).
    let mut sorted_tracks: Vec<RsTrackListRow> = results.tracks.clone();
    if sort.field != "rank" {
        sort_track_rows_by(
            &mut sorted_tracks,
            sort.field.as_str(),
            match sort.dir {
                SortDir::Asc => "asc",
                SortDir::Desc => "desc",
            },
            |t| t,
            |t| t.title.to_lowercase(),
        );
    } else if matches!(sort.dir, SortDir::Desc) {
        sorted_tracks.reverse();
    }

    let total = i32::try_from(sorted_tracks.len()).unwrap_or(i32::MAX);
    let top = compute_top_result(results, query);

    // Prepare the `Send` row halves here (worker thread on the fetch
    // path) so the event-loop closure below only pays for the `!Send`
    // cover lookups. Result sets are LIMIT-bounded, so preparing the
    // full set — rather than just the compact slice, which can't be
    // sized off-thread (`show-all-tracks` is UI-thread state) — is cheap.
    let prepared: Vec<PreparedTrackRow> = sorted_tracks
        .iter()
        .map(crate::ui::tracks::prepare_track_list_row)
        .collect();

    let album_rows: Vec<UiEntityStripRow> = results
        .albums
        .iter()
        .map(to_slint_album_strip_row)
        .collect();
    let artist_rows: Vec<UiEntityStripRow> = results
        .artists
        .iter()
        .map(|a| to_slint_artist_strip_row(a, &artist_subtitle(a)))
        .collect();

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Search>();

        // Songs — honour `show-all-tracks` against the sorted set.
        let show_all = g.get_show_all_tracks();
        let take = if show_all {
            prepared.len()
        } else {
            prepared.len().min(COMPACT_TRACK_LIMIT)
        };
        let mut rendered: Vec<UiTrackListRow> = prepared
            .into_iter()
            .take(take)
            .map(finish_track_list_row)
            .collect();
        restamp_rows(&g, &mut rendered);
        write_track_model(&g, rendered);
        g.set_tracks_total(total);

        // Strips.
        write_strip(&g.get_album_rows(), album_rows, "album");
        write_strip(&g.get_artist_rows(), artist_rows, "artist");

        // Top Result.
        if let Some(t) = top {
            g.set_top_kind(SharedString::from(match t.kind {
                TopKind::Album => "album",
                TopKind::Artist => "artist",
            }));
            g.set_top_id(crate::ui::util::clamp_i64_to_i32(t.id));
            g.set_top_title(SharedString::from(t.title.as_str()));
            g.set_top_subtitle(SharedString::from(t.subtitle.as_str()));
            g.set_top_artwork_path(SharedString::from(
                t.artwork_path.as_deref().unwrap_or(""),
            ));
        } else {
            g.set_top_kind(SharedString::from(""));
            g.set_top_id(-1);
            g.set_top_title(SharedString::from(""));
            g.set_top_subtitle(SharedString::from(""));
            g.set_top_artwork_path(SharedString::from(""));
        }
    });
}

/// Cached-results swap: re-derive the visible Songs slice from
/// `last_results` according to the *current* `show-all-tracks` flag
/// and active sort. Called from the `toggle-show-all-tracks` callback
/// — no DB hit; mirrors the apply step's slice/sort logic.
pub fn swap_tracks_compact_or_full(search_ui: &Arc<SearchUi>, weak: &Weak<AppWindow>) {
    let Some(results) = search_ui.state().last_results.lock().clone() else {
        return;
    };
    let query = search_ui.state().last_query.lock().clone();
    apply_results_to_slint(search_ui, weak, &results, &query);
}

/// 2-second delayed history add. The history token is bumped on every
/// keystroke (via the `query-changed` callback in `wire_search`); the
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
/// Called from `wire_search` on initial hydrate, after a successful
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

/// Compute the Top Result for a query against a `SearchResults`,
/// using a 6-step ranking. Pure function — exhaustively unit-tested.
///
/// Ranking (first match wins):
/// 1. Exact album name (case-insensitive)
/// 2. Exact artist name (case-insensitive)
/// 3. Album name starts-with (case-insensitive)
/// 4. Artist name starts-with (case-insensitive)
/// 5. First album in results
/// 6. First artist in results
///
/// Returns `None` only when both `results.albums` and `results.artists`
/// are empty.
pub fn compute_top_result(results: &SearchResults, query: &str) -> Option<TopResult> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }

    // 1. Exact album name
    if let Some(a) = results
        .albums
        .iter()
        .find(|a| a.name.to_lowercase() == needle)
    {
        return Some(album_to_top(a));
    }
    // 2. Exact artist name
    if let Some(a) = results
        .artists
        .iter()
        .find(|a| a.name.to_lowercase() == needle)
    {
        return Some(artist_to_top(a));
    }
    // 3. Album name starts-with
    if let Some(a) = results
        .albums
        .iter()
        .find(|a| a.name.to_lowercase().starts_with(&needle))
    {
        return Some(album_to_top(a));
    }
    // 4. Artist name starts-with
    if let Some(a) = results
        .artists
        .iter()
        .find(|a| a.name.to_lowercase().starts_with(&needle))
    {
        return Some(artist_to_top(a));
    }
    // 5. First album
    if let Some(a) = results.albums.first() {
        return Some(album_to_top(a));
    }
    // 6. First artist
    if let Some(a) = results.artists.first() {
        return Some(artist_to_top(a));
    }
    None
}

fn album_to_top(a: &AlbumStats) -> TopResult {
    TopResult {
        kind: TopKind::Album,
        id: a.id,
        title: a.name.clone(),
        subtitle: a.artist_name.clone(),
        artwork_path: a.artwork_path.clone(),
    }
}

fn artist_to_top(a: &ArtistStats) -> TopResult {
    TopResult {
        kind: TopKind::Artist,
        id: a.id,
        title: a.name.clone(),
        // Subtitle is the album count by Tauri parity. The localised
        // pluralisation is the caller's responsibility; we hand back
        // the raw count as a plain string so test fixtures stay stable.
        subtitle: format!("{} albums", a.album_count),
        artwork_path: a.image_path.clone(),
    }
}

/// Subtitle for an Artist strip card. The English fallback follows the
/// Tauri "{n} albums" shape; locale translation rides on the
/// `@tr("{n} album"|"{n} albums" % count)` plural in `.slint`. Until
/// the apply step can reach the locale machinery from a background
/// thread, the strip card subtitle is built in English here. That
/// matches the Favorites Artists strip's behaviour (see comment on
/// `to_slint_fav_artist_row`).
fn artist_subtitle(a: &ArtistStats) -> String {
    format!("{} albums", a.album_count)
}

/// Empty-query fast path: clear every results model + Top Result +
/// `loading`. Called synchronously from `kick_search` so the recent-
/// searches branch can render the same tick the user clears the
/// `SearchBar`.
pub fn clear_results_on_ui(weak: &Weak<AppWindow>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Search>();
        write_track_model(&g, Vec::new());
        write_strip(&g.get_album_rows(), Vec::new(), "album");
        write_strip(&g.get_artist_rows(), Vec::new(), "artist");
        g.set_tracks_total(0);
        g.set_top_kind(SharedString::from(""));
        g.set_top_id(-1);
        g.set_top_title(SharedString::from(""));
        g.set_top_subtitle(SharedString::from(""));
        g.set_top_artwork_path(SharedString::from(""));
        g.set_loading(false);
    });
}

/// Toggle the `loading` flag on the Slint side. Used by `kick_search`
/// to surface the "Searching…" line through the FTS round-trip.
pub fn set_loading_on_ui(weak: &Weak<AppWindow>, on: bool) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.global::<Search>().set_loading(on);
    });
}

/// UI-thread teardown — mirror of `release_section_state`'s Rust-side
/// cleanup, run synchronously *before* the `spawn_blocking` that drops
/// the LRUs. Clears every model + scalar + image slot the global owns
/// so the LRU's `SharedPixelBuffer` Arcs release on the same tick.
/// Keeps `Search.query` + `Search.recent-rows` so a brief flip away
/// and back doesn't lose the user's typing.
pub fn teardown_models_on_leave(ui: &AppWindow) {
    let g = ui.global::<Search>();
    write_track_model(&g, Vec::new());
    write_strip(&g.get_album_rows(), Vec::new(), "album");
    write_strip(&g.get_artist_rows(), Vec::new(), "artist");
    g.set_tracks_total(0);
    g.set_top_kind(SharedString::from(""));
    g.set_top_id(-1);
    g.set_top_title(SharedString::from(""));
    g.set_top_subtitle(SharedString::from(""));
    g.set_top_artwork_path(SharedString::from(""));
    g.set_loading(false);
    g.set_show_all_tracks(false);
    // Drop the selection set too so the next session enter starts
    // clean. The user just navigated away — they can't be relying on
    // a sticky selection.
    clear_selected_ids_model(&g);
    g.set_selection_anchor(-1);
    // Release the Image slot the Top Result card holds via its cover
    // callback — once `top-artwork-path` is empty the slot won't be
    // re-requested, so simply clearing the scalar is enough; no
    // explicit `Image::default()` write needed.
    let _ = Image::default();
}

fn clear_selected_ids_model(g: &Search) {
    let model = g.get_selected_ids();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(Vec::new());
    }
}

fn write_track_model(g: &Search, rows: Vec<UiTrackListRow>) {
    let model = g.get_tracks();
    let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() else {
        log::warn!("Search.tracks: VecModel<TrackListRow> downcast failed");
        return;
    };
    vm.set_vec(rows);
}

fn write_strip(
    model: &slint::ModelRc<UiEntityStripRow>,
    rows: Vec<UiEntityStripRow>,
    label: &str,
) {
    let Some(vm) = model.as_any().downcast_ref::<VecModel<UiEntityStripRow>>() else {
        log::warn!("Search.{label}-rows: VecModel<EntityStripRow> downcast failed");
        return;
    };
    vm.set_vec(rows);
}

#[cfg(test)]
#[path = "tests/top_result_tests.rs"]
mod tests;
