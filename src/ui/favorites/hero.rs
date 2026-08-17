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
use crate::ui::detail_artwork::DetailPair;
use crate::ui::detail_view::impl_detail_view_helpers;
use crate::{AppWindow, Favorites};

// Only the artwork half — this page's track model is not a detail `tracks` list.
impl_detail_view_helpers!(artwork_only Favorites);

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

    // A leave that landed while the query was in flight has already wiped
    // `stats` and emptied the mosaic-path model, so the store and the push
    // below would fill both back in behind a view nobody can see. The leave set
    // `mark_dirty`, so the next enter re-fetches — the guard `refresh_grids`
    // and `refresh_tracks` carry, in the same place, after the slow part.
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

    let Some(pair) = crate::ui::mosaic_hero::compose_off_thread(state, paths.clone()).await else {
        return Ok(());
    };
    publish_hero_artwork(fav_ui, weak, pair, animate, paths);
    Ok(())
}

/// Publish a composed banner and claim it as the one on screen.
///
/// **Gated whole, where a detail view fills its own slots even while hidden.** This page's
/// leave wipes its models and forgets the guard, so slots written behind it have nothing to
/// be ready for and their claim would suppress the re-enter's recompose. What the gate
/// mainly protects is still `HeroBackdrop`, shared by all six heroes: a compose finishing
/// after a nav away would paint this page's solve under whichever hero mounted next.
fn publish_hero_artwork(
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    pair: DetailPair,
    animate: bool,
    paths: Vec<String>,
) {
    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !fav_ui.section_active() || !fav_ui.state().last_mosaic_paths.claim(paths) {
            return;
        }
        apply_detail_artwork(&ui, &ui.global::<Favorites>(), pair, animate, true);
    });
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

/// Re-publish the band's chips on the UI thread.
///
/// **Call this wherever one of the chips' inputs lands.** Favorites is the one
/// hero built from three fetches rather than one: the stats, the Songs spread,
/// the Most Played totals — and `kick_full_refresh` runs those *concurrently*,
/// so no ordering can be assumed. Each fetch folds its own answer on its own
/// worker and then calls this; the publish itself reads only finished values,
/// so the worst a mistimed one can be is a tick behind, never half-built.
///
/// The grid path can't stand in for it: `write_filtered_grids` publishes past a
/// signature early-return, and `mounted_content` is a constant `0` on the Songs
/// tab — so on Songs that publish fires only when the column count moves.
pub fn republish_chips(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let fav_ui = fav_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::hero_chips::publish_favorites(&ui, &fav_ui);
    });
}
