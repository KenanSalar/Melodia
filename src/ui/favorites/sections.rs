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
//! Only the *mounted* tab's model is built. The tabs are mutually
//! exclusive, and a hidden grid's rows are `SharedString`s nobody can see.

use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use super::{FavoritesTab, FavoritesUi, tab_from_index, to_slint_fav_artist_row,
    to_slint_most_played_row};
use crate::library;
use crate::state::AppState;
use crate::ui::detail_filter::{field_contains, most_played_matches};
use crate::{
    AppWindow, EntityGridRow as UiEntityGridRow, EntityStripRow as UiEntityStripRow, Favorites,
};

/// Intermediate carrier for a filtered Favorite-Artist row crossing the
/// tokio→UI thread boundary. The Slint `Favorites.artist-favorite-subtitle`
/// pure callback that resolves the translated "{n} favorite[s]" subtitle
/// only runs on the UI thread, so the grid-applier builds these tuples
/// on the worker and finalises them inside `invoke_from_event_loop`.
struct FilteredArtistRow {
    artist: crate::entities::artist::FavoriteArtist,
}

/// Fetch Most Played + Favorite Artists in parallel and apply each
/// independently. Returns `()` because callers (`kick_full_refresh`)
/// have no use for a propagated error — every failure is logged here
/// with section context.
pub async fn refresh_grids(state: &AppState, fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let (most_played_res, fav_artists_res) = tokio::join!(
        library::favorites::get_most_played_favorites(state, None),
        library::favorites::get_favorite_artists(state),
    );

    match most_played_res {
        Ok(most_played) => {
            *fav_ui.state().most_played.lock() = most_played;
        }
        Err(e) => log::warn!("favorites::refresh_grids most_played: {e}"),
    }

    match fav_artists_res {
        Ok(fav_artists) => {
            *fav_ui.state().fav_artists.lock() = fav_artists;
        }
        Err(e) => log::warn!("favorites::refresh_grids fav_artists: {e}"),
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
    let warmed = fav_ui.section_active();
    if warmed {
        let fu = fav_ui.clone();
        let tab = fav_ui.active_tab();
        let _ = tokio::task::spawn_blocking(move || fu.prewarm_tab_covers(tab)).await;
    }

    // Both fetches resolved (or logged); push the filtered model so the
    // visible tab reflects fresh data AND the live filter in one pass.
    apply_filtered_grids(fav_ui, weak, warmed);
}

/// Both grids' filtered rows, prepared away from the UI thread by
/// [`build_filtered_grids`] and consumed by [`write_filtered_grids`].
struct PreparedGrids {
    most_played: Vec<UiEntityStripRow>,
    artists: Vec<FilteredArtistRow>,
}

/// Re-walk the cached `most_played` + `fav_artists` Rust Vecs through the
/// current `Favorites.filter`. Empty filter ⇒ all rows; non-empty ⇒
/// case-insensitive substring match on title+artist (Most Played) / name
/// (Artists). Runs entirely in memory and touches no Slint state, so either
/// thread can call it.
fn build_filtered_grids(fav_ui: &FavoritesUi) -> PreparedGrids {
    let needle = fav_ui.state().filter.lock().to_lowercase();

    let most_played: Vec<UiEntityStripRow> = {
        let cache = fav_ui.state().most_played.lock();
        cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
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
            .map(|a| FilteredArtistRow { artist: a.clone() })
            .collect()
    };

    PreparedGrids { most_played, artists }
}

/// Chunk the prepared rows into cards and push them into the mounted tab's
/// model, emptying the other. UI thread only.
///
/// The counts are published unchunked beside the models, for *both* tabs:
/// `rows.length` is a row count where the hero's stats line and the
/// empty-state gate want cards, and the hidden tab's count still has to be
/// right the moment it is picked.
fn write_filtered_grids(ui: &AppWindow, prepared: &PreparedGrids) {
    let g = ui.global::<Favorites>();

    g.set_most_played_count(len_as_i32(prepared.most_played.len()));
    g.set_artist_count(len_as_i32(prepared.artists.len()));

    let columns = g.get_columns();
    let tab = tab_from_index(&g, g.get_tab_idx());

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

/// Apply from a worker thread, hopping to the event loop to write.
///
/// `covers_warm` says the mounted tab's tier has already been decoded, which is
/// what lets the cards' bindings load on a miss again. It rides in the same
/// closure as the rows so a grid can never mount against a bumped counter and a
/// tier nobody warmed — the case [`refresh_grids`] hits when the user leaves the
/// section while its two queries are still in flight.
fn apply_filtered_grids(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>, covers_warm: bool) {
    let prepared = build_filtered_grids(fav_ui);
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        write_filtered_grids(&ui, &prepared);
        if covers_warm {
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
    write_filtered_grids(ui, &build_filtered_grids(fav_ui));
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
