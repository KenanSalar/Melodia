//! Cached entities → Slint models: the filtered walk, the chunk, and the two
//! ways an apply reaches the UI thread.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::warm::{grid_signature, mounted_content, should_announce_warm};
use crate::entities::artist::FavoriteArtist;
use crate::ui::favorites::{
    FavoritesTab, FavoritesUi, tab_from_index, to_slint_fav_artist_row, to_slint_most_played_row,
};
use crate::ui::grid_rows::chunk_rows;
use crate::ui::row_match::most_played_matches;
use crate::ui::util::len_as_i32;
use crate::{
    AppWindow, EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow, Favorites,
};

/// The mounted grid's filtered rows, prepared away from the UI thread by
/// [`build_filtered_grids`] and consumed by [`write_filtered_grids`].
pub(super) struct PreparedGrids {
    /// Which tab these rows were built for. Carried rather than re-derived
    /// because [`build_filtered_grids`] may run on a worker and
    /// [`write_filtered_grids`] always runs on the UI thread, so a pick can land
    /// in the gap — the same shape as `warmed_tab`, and checked the same way.
    tab: FavoritesTab,
    /// Empty unless [`Self::tab`] is `MostPlayed`.
    pub(super) most_played: Vec<UiEntityStripRow>,
    /// Empty unless [`Self::tab`] is `Artists`. Still entities rather than rows:
    /// the Slint `Favorites.artist-favorite-subtitle` callback that resolves the
    /// translated "{n} favorite[s]" line only runs on the UI thread, so these
    /// are finished inside `invoke_from_event_loop` rather than here.
    pub(super) artists: Vec<FavoriteArtist>,
    /// Filtered counts for **both** tabs, unlike the rows. They gate the two
    /// `GridEmptyState`s and feed the hero's stats line, so the tab that isn't
    /// mounted still has to publish one — and counting costs nothing extra, the
    /// walk that hashes runs either way.
    pub(super) most_played_count: usize,
    pub(super) artists_count: usize,
    /// Per-tab hash of everything that reaches a card, taken from the **source**
    /// entities rather than the built rows — which is what lets the rows above
    /// be built for one tab while the signature stays answerable for both.
    /// `#[derive(Hash)]` keeps it complete when a field is added, where a
    /// hand-listed set would quietly go stale. One per tab, not one for both — a
    /// play-count flush changes Most Played's and must not force the Artists
    /// grid, which nothing about it affects, to rebuild too.
    pub(super) most_played_content: u64,
    pub(super) artists_content: u64,
}

/// Re-walk the cached `most_played` + `fav_artists` Rust Vecs through the
/// current `Favorites.filter`, hashing and counting the survivors as they go.
/// Empty filter ⇒ all rows; non-empty ⇒ the shared [`most_played_matches`] walk
/// (Most Played) / [`crate::ui::row_match::Needle::contains`] on the name
/// (Artists). Runs entirely in memory and touches no Slint state, so either
/// thread can call it.
///
/// **Both tabs are walked; only the mounted one's rows are built.** The two are
/// mutually exclusive `if`s, so a row built for the other reaches a grid nothing
/// can scroll and is dropped by [`write_filtered_grids`] — and on the Songs tab
/// that was true of both, since `mounted_content` is a constant `0` there. What
/// makes the split possible is that the hashes come off the *source* entities
/// rather than the built rows, so the walk still answers the signature for the
/// tab it didn't build. Which tab that is comes off the `FavoritesUi` shadow,
/// the only form of the answer a worker can read.
pub(super) fn build_filtered_grids(fav_ui: &FavoritesUi) -> PreparedGrids {
    let needle = fav_ui.state().filter.lock().clone();
    let tab = fav_ui.active_tab();
    let mut most_played_hasher = DefaultHasher::new();
    let mut artists_hasher = DefaultHasher::new();

    let (most_played, most_played_count) = {
        let cache = fav_ui.state().most_played.lock();
        let matching = cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .inspect(|t| t.hash(&mut most_played_hasher));
        if tab == FavoritesTab::MostPlayed {
            let rows: Vec<UiEntityStripRow> = matching.map(to_slint_most_played_row).collect();
            let count = rows.len();
            (rows, count)
        } else {
            (Vec::new(), matching.count())
        }
    };

    // Artist rows can't be finished here — the subtitle is a translated
    // plural ("{n} favorite[s]") that only `Favorites.artist-favorite-subtitle`
    // resolves, and that is a UI-thread callback. Clone the filtered slice so
    // the source Mutex isn't held past this function.
    let (artists, artists_count) = {
        let cache = fav_ui.state().fav_artists.lock();
        let matching = cache
            .iter()
            .filter(|a| needle.contains(&a.name))
            .inspect(|a| a.hash(&mut artists_hasher));
        if tab == FavoritesTab::Artists {
            let rows: Vec<FavoriteArtist> = matching.cloned().collect();
            let count = rows.len();
            (rows, count)
        } else {
            (Vec::new(), matching.count())
        }
    };

    PreparedGrids {
        tab,
        most_played,
        artists,
        most_played_count,
        artists_count,
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
/// Three things short-circuit it. A hidden section is never written to — the
/// leave teardown emptied these models deliberately, and refilling them behind
/// it holds a card row per entity for a view nobody can see. An apply carrying
/// another tab's rows is dropped: [`build_filtered_grids`] materializes only the
/// mounted tab, so a pick landing between the build and this write would empty
/// the grid it just filled — and there is nothing to salvage, because that pick
/// ran [`apply_filtered_grids_now`] synchronously against the same caches on its
/// way through. And an apply that would repaint what is already on screen is
/// dropped: [`write_grid`] is a `set_vec` reset, so it tears down and rebuilds
/// every mounted card, and a `stats_changed` tick reaches both tabs while only
/// Most Played is ranked by play count.
fn write_filtered_grids(ui: &AppWindow, fav_ui: &FavoritesUi, prepared: &PreparedGrids) {
    if !fav_ui.section_active() {
        return;
    }

    let g = ui.global::<Favorites>();
    let columns = g.get_columns();
    let tab = tab_from_index(&g, g.get_tab_idx());
    if tab != prepared.tab {
        return;
    }

    let signature = grid_signature(tab, columns, mounted_content(tab, prepared));
    if fav_ui.state().last_grid_signature.lock().replace(signature) == Some(signature) {
        return;
    }

    g.set_most_played_count(len_as_i32(prepared.most_played_count));
    g.set_artist_count(len_as_i32(prepared.artists_count));
    // Covers a tab pick as well as a count change, because the signature above
    // hashes the tab — so anything that moves what the band should say has
    // already got past that early return.
    crate::ui::hero_chips::publish_favorites(ui, fav_ui);

    // The unmounted tab is emptied rather than left holding its last rows:
    // keeping them would pin one `SharedString` per field of every card behind a
    // tab the user has left. No branch is needed to do it — `build_filtered_grids`
    // materialized only the mounted tab, so the other's Vec is already empty and
    // chunks to nothing.
    write_grid(
        &g.get_most_played_rows(),
        chunk_entity_rows(&prepared.most_played, columns),
        "most-played",
    );

    let artist_rows: Vec<UiEntityStripRow> = prepared
        .artists
        .iter()
        .map(|a| to_slint_fav_artist_row(a, g.invoke_artist_favorite_subtitle(a.favorite_count)))
        .collect();
    write_grid(&g.get_artist_rows(), chunk_entity_rows(&artist_rows, columns), "artist");
}

/// Apply from a worker thread, hopping to the event loop to write.
///
/// `warmed_tab` is the tab whose tier `fetch::refresh_grids` decoded, and it
/// rides in the same closure as the rows so a grid can never mount against a
/// bumped counter and a tier nobody warmed — the case `refresh_grids` hits when
/// the user leaves the section while its two queries are still in flight.
pub(super) fn apply_filtered_grids(
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

/// Chunk a flat card list into rows of `columns` — the cards are already
/// built here, where the four entity grids project theirs out of a `GridData`
/// as they chunk.
fn chunk_entity_rows(rows: &[UiEntityStripRow], columns: i32) -> Vec<UiEntityGridRow> {
    chunk_rows(rows, columns, Clone::clone, |entities| UiEntityGridRow {
        entities,
    })
}

fn write_grid(model: &slint::ModelRc<UiEntityGridRow>, rows: Vec<UiEntityGridRow>, label: &str) {
    let Some(vec) = model.as_any().downcast_ref::<VecModel<UiEntityGridRow>>() else {
        log::warn!("Favorites.{label}-rows: VecModel<EntityGridRow> downcast failed");
        return;
    };
    vec.set_vec(rows);
}
