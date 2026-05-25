//! Strip fetches — Most Played (horizontal carousel) + Favorite
//! Artists (collapsible scroller). The two SQL queries are
//! independent and applied independently via `tokio::join!` (NOT
//! `try_join!`) so a failure on one section doesn't suppress the
//! other — a previous `try_join!` here silently hid both strips
//! whenever the Albums query errored, which was the root cause of
//! "the artists strip never showed up" before the album section was
//! dropped.
//!
//! Album-related state is gone by design — albums are reachable via
//! the Albums tab; surfacing them again on Favorites was redundant.
//!
//! Both strips honour the All Songs filter: every fetch / apply path
//! re-walks the cached Rust Vecs through the current `Favorites.filter`
//! needle (title+artist for Most Played, name for Favorite Artists)
//! before writing the Slint models. So the search bar in the hero
//! filters the strips and the tracklist in lockstep.

use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::{FavoritesUi, to_slint_fav_artist_row, to_slint_most_played_row};
use crate::library;
use crate::state::AppState;
use crate::{AppWindow, EntityStripRow as UiEntityStripRow, Favorites};

/// Intermediate carrier for a filtered Favorite-Artist row crossing the
/// tokio→UI thread boundary. The Slint `Favorites.artist-favorite-subtitle`
/// pure callback that resolves the translated "{n} favorite[s]" subtitle
/// only runs on the UI thread, so the strip-applier builds these tuples
/// on the worker and finalises them inside `invoke_from_event_loop`.
struct FilteredArtistRow {
    artist: crate::entities::artist::FavoriteArtist,
}

/// Cap for the Most Played Favorites strip. Matches the Tauri default —
/// enough to fill a horizontal scroll comfortably without inflating the
/// SQL projection.
const MOST_PLAYED_LIMIT: i64 = 10;

/// Fetch Most Played + Favorite Artists in parallel and apply each
/// independently. Returns `()` because callers (`kick_full_refresh`)
/// have no use for a propagated error — every failure is logged here
/// with section context.
pub async fn refresh_strips(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
) {
    let (most_played_res, fav_artists_res) = tokio::join!(
        library::favorites::get_most_played_favorites(state, MOST_PLAYED_LIMIT),
        library::favorites::get_favorite_artists(state),
    );

    match most_played_res {
        Ok(most_played) => {
            *fav_ui.state().most_played.lock() = most_played;
        }
        Err(e) => log::warn!("favorites::refresh_strips most_played: {e}"),
    }

    match fav_artists_res {
        Ok(fav_artists) => {
            *fav_ui.state().fav_artists.lock() = fav_artists;
        }
        Err(e) => log::warn!("favorites::refresh_strips fav_artists: {e}"),
    }

    // Both fetches resolved (or logged); push filtered models so the
    // strip-area reflects fresh data AND the live filter in one pass.
    apply_filtered_strips(fav_ui, weak);
}

/// Re-walk the cached `most_played` + `fav_artists` Rust Vecs through
/// the current `Favorites.filter` and push the resulting strip rows.
/// Runs entirely in memory; cheap enough to invoke on every keystroke
/// (the lists cap at 10 + however many favourite artists the library
/// has). Empty filter ⇒ all rows; non-empty ⇒ case-insensitive
/// substring match on title+artist (Most Played) / name (Artists).
pub fn apply_filtered_strips(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let needle = fav_ui.state().filter.lock().to_lowercase();

    let most_played_rows: Vec<UiEntityStripRow> = {
        let cache = fav_ui.state().most_played.lock();
        cache
            .iter()
            .filter(|t| {
                if needle.is_empty() {
                    return true;
                }
                if t.title.to_lowercase().contains(&needle) {
                    return true;
                }
                if let Some(a) = t.artist.as_deref()
                    && a.to_lowercase().contains(&needle)
                {
                    return true;
                }
                false
            })
            .map(to_slint_most_played_row)
            .collect()
    };

    // Defer artist-row materialisation to the UI thread — the subtitle
    // is a translated plural ("{n} favorite[s]") that has to resolve
    // through `Favorites.artist-favorite-subtitle(count)`. We clone the
    // small filtered slice (typically a few dozen entries) so the source
    // Mutex isn't held across the event-loop hop.
    let artist_filtered: Vec<FilteredArtistRow> = {
        let cache = fav_ui.state().fav_artists.lock();
        cache
            .iter()
            .filter(|a| needle.is_empty() || a.name.to_lowercase().contains(&needle))
            .map(|a| FilteredArtistRow { artist: a.clone() })
            .collect()
    };

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        let artist_rows: Vec<UiEntityStripRow> = artist_filtered
            .iter()
            .map(|f| {
                let subtitle = g.invoke_artist_favorite_subtitle(f.artist.favorite_count);
                to_slint_fav_artist_row(&f.artist, subtitle)
            })
            .collect();
        write_strip(&g.get_most_played_rows(), most_played_rows, "most-played");
        write_strip(&g.get_artist_rows(), artist_rows, "artist");
    });
}

fn write_strip(
    model: &slint::ModelRc<UiEntityStripRow>,
    rows: Vec<UiEntityStripRow>,
    label: &str,
) {
    let Some(vec) = model.as_any().downcast_ref::<VecModel<UiEntityStripRow>>() else {
        log::warn!("Favorites.{label}-rows: VecModel<EntityStripRow> downcast failed");
        return;
    };
    vec.set_vec(rows);
}
