//! Grid-tab fetches — Most Played and Favorite Artists. The two SQL
//! queries are independent and applied independently via `tokio::join!`
//! (NOT `try_join!`) so a failure on one tab doesn't suppress the other —
//! a previous `try_join!` here silently hid both surfaces whenever the
//! Albums query errored, which was the root cause of "the artists section
//! never showed up" before the album section was dropped.
//!
//! Album-related state is gone by design — albums are reachable via
//! the Albums tab; surfacing them again on Favorites was redundant.
//!
//! Neither query is capped: both tabs are virtualized `EntityCardGrid`s,
//! so the whole set is fetched once and only on-screen rows instantiate
//! cards. The cover prewarm *is* capped, at `GRID_PREWARM_AHEAD` — a
//! screenful, not the tier's capacity. Warming to capacity over an uncapped
//! grid means the prewarm evicts its own earlier work before a single card
//! asks for one; the rest decode lazily as rows scroll in.
//!
//! And only the mounted tab's tier, only while the section is on screen.
//! The two grids are mutually exclusive, so warming both is twice the
//! decodes and twice the resident buffers for a surface nobody can scroll.
//!
//! Both grids honour the hero filter: every fetch / apply path re-walks the
//! cached Rust Vecs through the current `Favorites.filter` needle (title+artist
//! for Most Played, name for Favorite Artists) before writing the Slint models.
//!
//! Favorite Artists also sorts, and it does so on the **cache** rather than on
//! that filtered walk — see [`sort_artists`]. Most Played doesn't sort at all:
//! its SQL rank is the tab's whole meaning.
//!
//! Only the *mounted* tab's model is built. The tabs are mutually
//! exclusive, and a hidden grid's rows are `SharedString`s nobody can see.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::{FavoritesTab, FavoritesUi, tab_from_index, to_slint_fav_artist_row,
    to_slint_most_played_row};
use crate::entities::artist::FavoriteArtist;
use crate::library;
use crate::services::settings::{SortDir, ViewSort};
use crate::state::AppState;
use crate::ui::row_match::{field_contains, most_played_matches};
use crate::{
    AppWindow, EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow, Favorites,
};

/// Intermediate carrier for a filtered Favorite-Artist row crossing the
/// tokio→UI thread boundary. The Slint `Favorites.artist-favorite-subtitle`
/// pure callback that resolves the translated "{n} favorite[s]" subtitle
/// only runs on the UI thread, so the grid-applier builds these tuples
/// on the worker and finalises them inside `invoke_from_event_loop`.
struct FilteredArtistRow {
    artist: FavoriteArtist,
}

/// Order the Favorite Artists cache in place.
///
/// The **cache**, not the filtered copy [`build_filtered_grids`] builds, because
/// [`FavoritesUi::first_screenful_paths`] reads the cache directly to decide
/// which covers to prewarm — sorting downstream would warm whichever artists SQL
/// happened to return first while the grid painted a different prefix. Filtering
/// preserves order, so one sort here serves both.
///
/// `favorite_count` breaks ties by name. The SQL it replaces broke them not at
/// all, so artists on the same count could swap places between refreshes.
///
/// Mirrors `ui::artists::grid::sort_artist_indices`, down to reversing rather
/// than branching the comparator.
pub(super) fn sort_artists(artists: &mut [FavoriteArtist], field: &str, dir: SortDir) {
    match field {
        "name" => artists.sort_by_cached_key(|a| a.name.to_lowercase()),
        _ => artists.sort_by_cached_key(|a| (a.favorite_count, a.name.to_lowercase())),
    }
    if matches!(dir, SortDir::Desc) {
        artists.reverse();
    }
}

/// Re-order the cached Favorite Artists to the active sort. Cheap, in-memory,
/// callable from either thread.
fn sort_cached_artists(fav_ui: &FavoritesUi) {
    // Clone the sort out in its own statement — taking the second lock while
    // the first guard is still live would nest them for no reason.
    let ViewSort { field, dir } = fav_ui.state().artist_sort.lock().clone();
    sort_artists(&mut fav_ui.state().fav_artists.lock(), &field, dir);
}

/// Set the Favorite Artists sort and re-order the cache to match.
///
/// One call rather than two so no path can move the shadow without moving the
/// rows the prewarm reads — which would be invisible until the covers came up
/// against the wrong cards.
pub fn set_artist_sort(fav_ui: &FavoritesUi, field: String, dir: SortDir) {
    *fav_ui.state().artist_sort.lock() = ViewSort { field, dir };
    sort_cached_artists(fav_ui);
}

/// Fetch Most Played + Favorite Artists in parallel and apply each
/// independently. Returns `()` because callers (`kick_full_refresh`)
/// have no use for a propagated error — every failure is logged here
/// with section context.
pub async fn refresh_grids(state: &AppState, fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let (most_played_res, fav_artists_res) = tokio::join!(
        library::favorites::get_most_played_favorites(state),
        library::favorites::get_favorite_artists(state),
    );

    // Logged before the guard below, not at the store — a query that failed is
    // worth a line whether or not anyone is still looking at the view.
    let most_played = most_played_res
        .inspect_err(|e| log::warn!("favorites::refresh_grids most_played: {e}"))
        .ok();
    let fav_artists = fav_artists_res
        .inspect_err(|e| log::warn!("favorites::refresh_grids fav_artists: {e}"))
        .ok();

    // A leave that landed while the two queries were in flight has already
    // cleared these caches (and emptied both models), so storing now would undo
    // the teardown behind a view nobody can see. Nothing is lost by dropping the
    // result: every leave sets `mark_dirty`, so the next enter re-fetches.
    if !fav_ui.section_active() {
        return;
    }

    if let Some(rows) = most_played {
        *fav_ui.state().most_played.lock() = rows;
    }
    if let Some(rows) = fav_artists {
        *fav_ui.state().fav_artists.lock() = rows;
        // Before the prewarm below, which reads this cache for its paths — the
        // query returns no order at all, so this is what puts the rows in the
        // one the tab is about to paint.
        sort_cached_artists(fav_ui);
    }

    // Prewarm the mounted tab's tier off-thread before its rows land in the
    // Slint model: the cards' `request-*-cover` lookups decode on miss *on
    // the UI thread*, so a cold tab would otherwise pay one synchronous
    // 448 px decode per visible card at first paint.
    //
    // Only the mounted tab, and only while the section is on screen. The two
    // grids are mutually exclusive, so warming both is twice the decodes and
    // twice the resident buffers for a surface nobody can scroll; and a
    // `library_changed` tick that arrives while Favorites is hidden has
    // already been turned into a `mark_dirty` by the caller, so there is
    // nothing on screen to warm for. The other tab warms in `tab-changed`.
    let warmed_tab = if fav_ui.section_active() {
        let fu = fav_ui.clone();
        let tab = fav_ui.active_tab();
        let _ = tokio::task::spawn_blocking(move || fu.prewarm_tab_covers(tab)).await;
        Some(tab)
    } else {
        None
    };

    // Both fetches resolved (or logged); push the filtered model so the
    // visible tab reflects fresh data AND the live filter in one pass.
    apply_filtered_grids(fav_ui, weak, warmed_tab);
}

/// Both grids' filtered rows, prepared away from the UI thread by
/// [`build_filtered_grids`] and consumed by [`write_filtered_grids`].
struct PreparedGrids {
    most_played: Vec<UiEntityStripRow>,
    artists: Vec<FilteredArtistRow>,
    /// Per-tab hash of everything that reaches a card, taken from the **source**
    /// entities rather than the built rows: `#[derive(Hash)]` keeps it complete
    /// when a field is added, where a hand-listed set would quietly go stale.
    /// One per tab, not one for both — a play-count flush changes Most Played's
    /// and must not force the Artists grid, which nothing about it affects, to
    /// rebuild too.
    most_played_content: u64,
    artists_content: u64,
}

/// Re-walk the cached `most_played` + `fav_artists` Rust Vecs through the
/// current `Favorites.filter`, hashing the survivors as they go. Empty filter ⇒
/// all rows; non-empty ⇒ the shared [`most_played_matches`] walk (Most Played) /
/// [`field_contains`] on the name (Artists). Runs entirely in memory and touches
/// no Slint state, so either thread can call it.
fn build_filtered_grids(fav_ui: &FavoritesUi) -> PreparedGrids {
    let needle = fav_ui.state().filter.lock().clone();
    let mut most_played_hasher = DefaultHasher::new();
    let mut artists_hasher = DefaultHasher::new();

    let most_played: Vec<UiEntityStripRow> = {
        let cache = fav_ui.state().most_played.lock();
        cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .inspect(|t| t.hash(&mut most_played_hasher))
            .map(to_slint_most_played_row)
            .collect()
    };

    // Artist rows can't be finished here — the subtitle is a translated
    // plural ("{n} favorite[s]") that only `Favorites.artist-favorite-subtitle`
    // resolves, and that is a UI-thread callback. Clone the filtered slice so
    // the source Mutex isn't held past this function.
    let artists: Vec<FilteredArtistRow> = {
        let cache = fav_ui.state().fav_artists.lock();
        cache
            .iter()
            .filter(|a| field_contains(&a.name, &needle))
            .inspect(|a| a.hash(&mut artists_hasher))
            .map(|a| FilteredArtistRow { artist: a.clone() })
            .collect()
    };

    PreparedGrids {
        most_played,
        artists,
        most_played_content: most_played_hasher.finish(),
        artists_content: artists_hasher.finish(),
    }
}

/// Chunk the prepared rows into cards and push them into the mounted tab's
/// model, emptying the other. UI thread only.
///
/// The counts are published unchunked beside the models, for *both* tabs:
/// `rows.length` is a row count where the hero's stats line and the
/// empty-state gate want cards. They ride along with the models rather than
/// being written unconditionally, which is safe only because every reader of a
/// count is inside that tab's own branch or gated on `tab-idx` — so a count
/// left stale under the skip below is one nothing can render, and picking the
/// tab is itself a signature change that refreshes it.
///
/// Two things short-circuit it. A hidden section is never written to — the
/// leave teardown emptied these models deliberately, and refilling them behind
/// it holds a card row per entity for a view nobody can see. And an apply that
/// would repaint what is already on screen is dropped: [`write_grid`] is a
/// `set_vec` reset, so it tears down and rebuilds every mounted card, and a
/// `stats_changed` tick reaches both tabs while only Most Played is ranked by
/// play count.
fn write_filtered_grids(ui: &AppWindow, fav_ui: &FavoritesUi, prepared: &PreparedGrids) {
    if !fav_ui.section_active() {
        return;
    }

    let g = ui.global::<Favorites>();
    let columns = g.get_columns();
    let tab = tab_from_index(&g, g.get_tab_idx());

    let signature = grid_signature(tab, columns, mounted_content(tab, prepared));
    if fav_ui.state().last_grid_signature.lock().replace(signature) == Some(signature) {
        return;
    }

    g.set_most_played_count(len_as_i32(prepared.most_played.len()));
    g.set_artist_count(len_as_i32(prepared.artists.len()));

    // Only the mounted tab's model is built, and the other is emptied
    // rather than left holding its last rows: building it would allocate
    // a card row per entity for a grid nothing can scroll, and keeping it
    // would pin one `SharedString` per field of every card behind a tab
    // the user has left.
    let on_most_played = tab == FavoritesTab::MostPlayed;
    let on_artists = tab == FavoritesTab::Artists;

    write_grid(
        &g.get_most_played_rows(),
        if on_most_played {
            chunk_entity_rows(&prepared.most_played, columns)
        } else {
            Vec::new()
        },
        "most-played",
    );

    let artist_rows: Vec<UiEntityStripRow> = if on_artists {
        prepared
            .artists
            .iter()
            .map(|f| {
                let subtitle = g.invoke_artist_favorite_subtitle(f.artist.favorite_count);
                to_slint_fav_artist_row(&f.artist, subtitle)
            })
            .collect()
    } else {
        Vec::new()
    };
    write_grid(&g.get_artist_rows(), chunk_entity_rows(&artist_rows, columns), "artist");
}

/// The content hash of the tab that is actually on screen.
///
/// Only that one can be what changed visibly — the hidden grid's model is empty
/// either way. Folding both in would undo the whole point of hashing them apart:
/// every play-count flush moves Most Played's hash, and the Artists grid, which
/// shows nothing derived from a play count, would rebuild along with it.
fn mounted_content(tab: FavoritesTab, prepared: &PreparedGrids) -> u64 {
    match tab {
        FavoritesTab::MostPlayed => prepared.most_played_content,
        FavoritesTab::Artists => prepared.artists_content,
        FavoritesTab::Songs => 0,
    }
}

/// Fold the mounted tab and the column count into the content hash.
///
/// Both shape what is on screen independently of the data: a tab switch has to
/// fill one model and empty the other, and a column change re-chunks the same
/// cards into different rows. Leave either out and the apply that needs to run
/// most is the one that gets skipped.
fn grid_signature(tab: FavoritesTab, columns: i32, content: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    tab.hash(&mut hasher);
    columns.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

/// Whether a landed prewarm may announce its tier to the cards.
///
/// `warmed` is the tab [`refresh_grids`] actually decoded for, `None` when it
/// skipped the prewarm because the section was already hidden. The other two
/// are read on the UI thread, where both shadows are written, so this is the
/// same re-check `on_tab_changed` makes after *its* `swap_tab_covers`: a leave
/// has rewound the counter and dropped the buffers, and a tab pick that
/// overtook the decodes owns a different tier entirely — announcing either
/// would put the next surface's cards straight back on the decoding path.
///
/// Deliberately *not* a function of whether the rows changed. Those are
/// independent facts, and conflating them is what left the Most Played grid on
/// placeholders after a section re-enter: the mount-time `columns-changed`
/// apply had already written the final rows by the time the prewarm returned,
/// so the write that carried the announcement was skipped as a no-op repaint
/// and the counter stayed at its cold 0 until the next tab pick.
fn should_announce_warm(
    warmed: Option<FavoritesTab>,
    section_active: bool,
    current_tab: FavoritesTab,
) -> bool {
    section_active && warmed == Some(current_tab)
}

/// Apply from a worker thread, hopping to the event loop to write.
///
/// `warmed_tab` is the tab whose tier [`refresh_grids`] decoded, and it rides in
/// the same closure as the rows so a grid can never mount against a bumped
/// counter and a tier nobody warmed — the case [`refresh_grids`] hits when the
/// user leaves the section while its two queries are still in flight.
fn apply_filtered_grids(
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    warmed_tab: Option<FavoritesTab>,
) {
    let prepared = build_filtered_grids(fav_ui);
    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        write_filtered_grids(&ui, &fav_ui, &prepared);
        if should_announce_warm(warmed_tab, fav_ui.section_active(), fav_ui.active_tab()) {
            mark_covers_warm(&ui);
        }
    });
}

/// Apply from the UI thread, with no event-loop hop — the rows land in the
/// model before Slint re-evaluates the `if` that mounts the entering tab.
///
/// Posting them instead races the redraw, and a redraw that wins paints a bare
/// panel: the hidden tab's model is emptied on every apply, and its
/// `GridEmptyState` is suppressed by a count that is already non-zero. Mirrors
/// `ui::albums::grid::rebuild_grid`, which is a plain call for the same reason.
pub fn apply_filtered_grids_now(ui: &AppWindow, fav_ui: &FavoritesUi) {
    write_filtered_grids(ui, fav_ui, &build_filtered_grids(fav_ui));
}

/// Let the mounted grid's card bindings start decoding on a miss again — see
/// `Favorites.covers-generation`.
pub fn mark_covers_warm(ui: &AppWindow) {
    let g = ui.global::<Favorites>();
    g.set_covers_generation(g.get_covers_generation().saturating_add(1));
}

/// Chunk a flat card list into rows of `columns`. Mirrors
/// `ui::artists::grid::chunk_indices` — the `ListView` inside
/// `EntityCardGrid` virtualizes by row, so the chunking *is* the
/// virtualization boundary.
fn chunk_entity_rows(rows: &[UiEntityStripRow], columns: i32) -> Vec<UiEntityGridRow> {
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let mut grid_rows: Vec<UiEntityGridRow> = Vec::with_capacity(rows.len().div_ceil(cols));
    for chunk in rows.chunks(cols) {
        grid_rows.push(UiEntityGridRow {
            entities: ModelRc::from(Rc::new(VecModel::from(chunk.to_vec()))),
        });
    }
    grid_rows
}

/// Slint counts are `i32`; a library that overflows one has bigger problems
/// than a wrong stats line, so saturate rather than wrap.
fn len_as_i32(len: usize) -> i32 {
    i32::try_from(len).unwrap_or(i32::MAX)
}

fn write_grid(model: &slint::ModelRc<UiEntityGridRow>, rows: Vec<UiEntityGridRow>, label: &str) {
    let Some(vec) = model.as_any().downcast_ref::<VecModel<UiEntityGridRow>>() else {
        log::warn!("Favorites.{label}-rows: VecModel<EntityGridRow> downcast failed");
        return;
    };
    vec.set_vec(rows);
}

#[cfg(test)]
#[path = "tests/sections_tests.rs"]
mod tests;
