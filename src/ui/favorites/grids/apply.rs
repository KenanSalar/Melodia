//! Cached entities → Slint models: the filtered walk, the chunk, and the two
//! ways an apply reaches the UI thread.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::warm::mounted_content;
use crate::entities::artist::FavoriteArtist;
use crate::ui::favorites::{
    FavoritesTab, FavoritesUi, tab_from_index, to_slint_fav_artist_row, to_slint_most_played_row,
};
use crate::ui::grid_rows::{chunk_entity_rows, write_grid};
use crate::ui::row_match::most_played_matches;
use crate::ui::tab_bar::{grid_signature, should_announce_warm};
use crate::ui::util::len_as_i32;
use crate::{AppWindow, EntityStripRow as UiEntityStripRow, Favorites};

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
    /// translated `"{n} favorite[s]"` line only runs on the UI thread, so these
    /// are finished inside `invoke_from_event_loop` rather than here.
    pub(super) artists: Vec<FavoriteArtist>,
    /// Filtered counts gating the two `GridEmptyState`s. `0` on an unwalked tab,
    /// where nothing reads them — the band takes its facts from
    /// `FavoritesUiState`'s folds instead.
    pub(super) most_played_count: usize,
    pub(super) artists_count: usize,
    /// Per-tab hash of everything reaching a card, off the **source** entities
    /// rather than the built rows: `#[derive(Hash)]` stays complete when a field
    /// is added, where a hand-listed set goes quietly stale. One per tab, not
    /// one for both — a play-count flush moves Most Played's and must not force
    /// the Artists grid, which it doesn't touch, to rebuild.
    pub(super) most_played_content: u64,
    pub(super) artists_content: u64,
}

/// Re-walk the cached `most_played` and `fav_artists` through the current
/// filter, hashing and counting the survivors as they go. Entirely in memory and
/// touching no Slint state, so either thread can call it — the mounted tab comes
/// off the `FavoritesUi` shadow, the only form of that answer a worker can read.
///
/// **Only the mounted tab's cache is walked at all.** The three sub-views are
/// mutually exclusive `if`s, so an unmounted grid's rows, count and hash all
/// reach nothing — `mounted_content` already answers a constant `0` for the two
/// that aren't up, and each count's readers sit inside its own branch. Walking
/// both cost a fold of the needle against every cached entity plus a
/// string-heavy `Hash` of each survivor, and `apply_filtered_grids_now` reaches
/// this **on the UI thread**.
pub(super) fn build_filtered_grids(fav_ui: &FavoritesUi) -> PreparedGrids {
    let needle = fav_ui.state().filter.lock().clone();
    let tab = fav_ui.active_tab();

    let (most_played, most_played_content) = if tab == FavoritesTab::MostPlayed {
        let mut hasher = DefaultHasher::new();
        let rows: Vec<UiEntityStripRow> = fav_ui
            .state()
            .most_played
            .lock()
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .inspect(|t| t.hash(&mut hasher))
            .map(to_slint_most_played_row)
            .collect();
        (rows, hasher.finish())
    } else {
        (Vec::new(), 0)
    };

    // Artist rows can't be finished here — the subtitle is a translated plural
    // only a UI-thread callback resolves. Cloning the filtered slice is what
    // keeps the source `Mutex` from being held past this function.
    let (artists, artists_content) = if tab == FavoritesTab::Artists {
        let mut hasher = DefaultHasher::new();
        let rows: Vec<FavoriteArtist> = fav_ui
            .state()
            .fav_artists
            .lock()
            .iter()
            .filter(|a| needle.contains(&a.name))
            .inspect(|a| a.hash(&mut hasher))
            .cloned()
            .collect();
        (rows, hasher.finish())
    } else {
        (Vec::new(), 0)
    };

    let most_played_count = most_played.len();
    let artists_count = artists.len();
    PreparedGrids {
        tab,
        most_played,
        artists,
        most_played_count,
        artists_count,
        most_played_content,
        artists_content,
    }
}

/// Chunk the prepared rows into cards and push them into the mounted tab's
/// model, emptying the other. UI thread only, and takes `prepared` **by value**
/// so the rows move into the per-row models rather than being cloned into them.
///
/// The counts are published unchunked beside the models, `rows.length` being a
/// row count where the two `GridEmptyState`s want cards. Only the mounted tab's
/// stands for anything, the other being a constant `0`; that is safe because
/// every reader sits inside its own tab's branch or under a `tab-idx` gate, and
/// picking that tab is itself a signature change that recomputes it.
///
/// **They are written above the signature guard, and that placement is what
/// keeps a tab pick's `UNFETCHED_COUNT` rewind from being permanent.** The pick
/// stamps a signature against the cache a skipped tick left in place and *then*
/// rewinds the count; when the fetch it spawned returns the same content the
/// guard fires and the sentinel is left with nothing coming. `-1` matches
/// neither `> 0` nor `== 0`, so what that hides is not just the empty state —
/// the Shuffle pill and the sort row are gated the other way and vanish over a
/// full grid. When the guard fires the model already holds exactly `prepared`,
/// so the counts above it are by construction the ones it shows. And hoisting
/// costs *nothing*, not merely little: `Property::set` is value-compared, so on
/// the path the guard would have taken nothing is stored and no dependent is
/// marked dirty.
///
/// Three things short-circuit it. A hidden section is never written to — the
/// leave teardown emptied these models deliberately, and refilling them holds a
/// card row per entity for a view nobody can see. An apply carrying another
/// tab's rows is dropped, [`build_filtered_grids`] materializing only the
/// mounted tab, so a pick landing in the gap would empty the grid it just filled
/// — and nothing is lost, that pick having run [`apply_filtered_grids_now`]
/// synchronously against the same caches. And an apply that would repaint what
/// is already on screen is dropped, [`write_grid`] being a `set_vec` reset that
/// tears down every mounted card, where a `stats_changed` tick reaches both tabs
/// and only ranks one.
fn write_filtered_grids(ui: &AppWindow, fav_ui: &FavoritesUi, prepared: PreparedGrids) {
    if !fav_ui.section_active() {
        return;
    }

    let g = ui.global::<Favorites>();
    let columns = g.get_columns();
    let tab = tab_from_index(&g, g.get_tab_idx());
    if tab != prepared.tab {
        return;
    }

    // Above the guard — see the doc comment.
    g.set_most_played_count(len_as_i32(prepared.most_played_count));
    g.set_artist_count(len_as_i32(prepared.artists_count));

    let signature = grid_signature(tab, columns, mounted_content(tab, &prepared));
    if fav_ui.state().last_grid_signature.lock().replace(signature) == Some(signature) {
        return;
    }

    // Covers a tab pick as well as a count change, the signature above hashing
    // the tab — so anything that moves what the band says is already past the
    // early return.
    crate::ui::hero_chips::publish_favorites(ui, fav_ui);

    // The unmounted tab is emptied rather than left pinning a `SharedString` per
    // field of every card behind a tab the user has left. No branch needed:
    // `build_filtered_grids` materialized only the mounted tab, so the other's
    // Vec is already empty and chunks to nothing.
    write_grid(
        &g.get_most_played_rows(),
        chunk_entity_rows(prepared.most_played, columns),
        "Favorites.most-played-rows",
    );

    let artist_rows: Vec<UiEntityStripRow> = prepared
        .artists
        .iter()
        .map(|a| to_slint_fav_artist_row(a, g.invoke_artist_favorite_subtitle(a.favorite_count)))
        .collect();
    write_grid(
        &g.get_artist_rows(),
        chunk_entity_rows(artist_rows, columns),
        "Favorites.artist-rows",
    );
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
        write_filtered_grids(&ui, &fav_ui, prepared);
        if should_announce_warm(warmed_tab, fav_ui.section_active(), fav_ui.active_tab()) {
            mark_covers_warm(&ui);
        }
    });
}

/// Apply from the UI thread, with no event-loop hop — the rows land in the
/// model before Slint re-evaluates the `if` that mounts the entering tab.
///
/// Posting them races the redraw, and a redraw that wins paints a bare panel:
/// the hidden tab's model is emptied on every apply, and its `GridEmptyState` is
/// suppressed by a count that is already non-zero. `ui::albums::grid::rebuild_grid`
/// is a plain call for the same reason.
pub fn apply_filtered_grids_now(ui: &AppWindow, fav_ui: &FavoritesUi) {
    write_filtered_grids(ui, fav_ui, build_filtered_grids(fav_ui));
}

/// Let the mounted grid's card bindings start decoding on a miss again — see
/// `Favorites.covers-generation`.
pub fn mark_covers_warm(ui: &AppWindow) {
    let g = ui.global::<Favorites>();
    g.set_covers_generation(g.get_covers_generation().saturating_add(1));
}
