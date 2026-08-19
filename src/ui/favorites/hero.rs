//! Hero stats + the banner artwork refresh.
//!
//! On every `library_changed_tx` tick the Favorites view is visible for, this re-fetches
//! `library::favorites::get_favorite_stats` and composes the top-4 most-played covers into
//! one collage through [`crate::ui::mosaic_hero`]. Past that the banner is an ordinary
//! single-artwork hero: `apply_detail_artwork` writes the cover slot directly and the blur
//! through `write_crossfade_slot`, so switching collages fades rather than flashing.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::FavoritesUi;
use crate::entities::track::FavoriteStats;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::detail_view::impl_detail_view_helpers;
use crate::{AppWindow, Favorites};

// Only the artwork half — this page's track model is not a detail `tracks` list.
impl_detail_view_helpers!(curated Favorites, FavoritesUi, crate::ui::hero_chips::publish_favorites);

/// Fetch fresh stats, push the count and the band's chips with it, then kick a blocking
/// compose whose result lands on the UI thread via `invoke_from_event_loop`. `animate`
/// fades the cross-fade between the old banner blur and the new.
///
/// The running time reaches the band as a chip rather than as a property: the
/// millisecond total is already on the stats struct, so routing a formatted
/// string through Slint only to read it back would be a round trip for
/// something this crate had in hand.
pub async fn refresh_hero(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    animate: bool,
) -> AppResult<()> {
    let stats = library::favorites::get_favorite_stats(state).await?;

    // A leave that landed while the query was in flight has already wiped `stats` and
    // forgotten the collage guard, so the store and the push below would fill both back
    // in behind a view nobody can see. The leave set `mark_dirty`, so the next enter
    // re-fetches — the guard `refresh_grids` and `refresh_tracks` carry, in the same
    // place, after the slow part.
    if !fav_ui.section_active() {
        return Ok(());
    }

    {
        // Under the section gate: `release_section_state` wipes this beside the
        // caches, so the two must not interleave. Synchronous store only.
        let _gate = fav_ui.gate();
        *fav_ui.state().stats.lock() = stats.clone();
    }
    push_stats_to_slint(&stats, fav_ui, weak);

    let paths = stats.artwork_paths.clone();

    // Skip the compose when those covers are already the ones on screen — a library or
    // stats tick usually returns the same top four, and an unchanged banner is still
    // correct. The matching *claim* is in `publish_hero_artwork`, for `MosaicGuard`'s
    // reason.
    if !fav_ui.state().last_mosaic_paths.is_stale(&paths) {
        return Ok(());
    }

    let Some(pair) =
        crate::ui::mosaic_hero::compose_off_thread(state, paths.clone(), fav_ui.hero_blur).await
    else {
        return Ok(());
    };
    publish_hero_artwork(fav_ui, weak, pair, animate, paths);
    Ok(())
}

fn push_stats_to_slint(stats: &FavoriteStats, fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let count = i32::try_from(stats.count).unwrap_or(i32::MAX);
    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        // The leave can land while this post is in flight, and it rewinds the count on its
        // way out — the same guard, in the same place, as `songs::apply_filtered_tracks`.
        if !fav_ui.section_active() {
            return;
        }
        ui.global::<Favorites>().set_track_count(count);
        // Order-free: the chips take their facts off the handle's own state,
        // not back off the properties written around them.
        crate::ui::hero_chips::publish_favorites(&ui, &fav_ui);
    });
}
