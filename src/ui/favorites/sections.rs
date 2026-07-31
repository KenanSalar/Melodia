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
    if fav_ui.section_active() {
        let fu = fav_ui.clone();
        let tab = fav_ui.active_tab();
        let _ = tokio::task::spawn_blocking(move || fu.prewarm_tab_covers(tab)).await;
    }

    // Both fetches resolved (or logged); push the filtered model so the
    // visible tab reflects fresh data AND the live filter in one pass.
    apply_filtered_grids(fav_ui, weak);
}

/// Re-walk the cached `most_played` + `fav_artists` Rust Vecs through
/// the current `Favorites.filter`, chunk the survivors into card rows,
/// and push the result into the mounted tab's model (emptying the other).
/// Runs entirely in memory. Empty filter ⇒ all rows; non-empty ⇒
/// case-insensitive substring match on title+artist (Most Played) / name
/// (Artists).
///
/// The counts are published unchunked beside the models, for *both* tabs:
/// `rows.length` is a row count where the hero's stats line and the
/// empty-state gate want cards, and the hidden tab's count still has to be
/// right the moment it is picked.
pub fn apply_filtered_grids(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let needle = fav_ui.state().filter.lock().to_lowercase();

    let most_played_rows: Vec<UiEntityStripRow> = {
        let cache = fav_ui.state().most_played.lock();
        cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .map(to_slint_most_played_row)
            .collect()
    };

    // Defer artist-row materialisation to the UI thread — the subtitle
    // is a translated plural ("{n} favorite[s]") that has to resolve
    // through `Favorites.artist-favorite-subtitle(count)`. We clone the
    // small filtered slice so the source Mutex isn't held across the
    // event-loop hop.
    let artist_filtered: Vec<FilteredArtistRow> = {
        let cache = fav_ui.state().fav_artists.lock();
        cache
            .iter()
            .filter(|a| field_contains(&a.name, &needle))
            .map(|a| FilteredArtistRow { artist: a.clone() })
            .collect()
    };

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();

        g.set_most_played_count(len_as_i32(most_played_rows.len()));
        g.set_artist_count(len_as_i32(artist_filtered.len()));

        let columns = g.get_columns();
        let tab = tab_from_index(&g, g.get_tab_idx());

        // Only the mounted tab's model is built, and the other is emptied
        // rather than left holding its last rows: building it would allocate
        // a card row per entity for a grid nothing can scroll, and keeping it
        // would pin one `SharedString` per field of every card behind a tab
        // the user has left. `tab-changed` calls straight back in here, so
        // the tab being entered is filled on the same tick it is mounted.
        let on_most_played = tab == FavoritesTab::MostPlayed;
        let on_artists = tab == FavoritesTab::Artists;

        write_grid(
            &g.get_most_played_rows(),
            if on_most_played { chunk_entity_rows(&most_played_rows, columns) } else { Vec::new() },
            "most-played",
        );

        let artist_rows: Vec<UiEntityStripRow> = if on_artists {
            artist_filtered
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
    });
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
