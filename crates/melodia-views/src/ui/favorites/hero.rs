//! The band's stats and its collage, composed through [`crate::ui::mosaic_hero`] and then
//! painted like any single-artwork hero.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::FavoritesUi;
use crate::ui::detail_view::impl_curated_hero_helpers;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::track::FavoriteStats;
use melodia_core::error::AppResult;
use melodia_ui::{AppWindow, Favorites};

impl_curated_hero_helpers!(Favorites, FavoritesUi);

/// Fetches the stats, publishes the count and chips, and recomposes the collage when its covers
/// changed. `animate` cross-fades the banner from the old one.
pub async fn refresh_hero(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    animate: bool,
) -> AppResult<()> {
    let stats = library::favorites::get_favorite_stats(&state.db).await?;

    // A leave landing mid-query has already wiped `stats` and the collage guard, and the next
    // enter re-fetches.
    if !fav_ui.section_active() {
        return Ok(());
    }

    {
        let _gate = fav_ui.gate();
        *fav_ui.state().stats.lock() = stats.clone();
    }
    push_stats_to_slint(&stats, fav_ui, weak);

    let paths = stats.artwork_paths.clone();

    // A tick usually returns the same top four. The matching claim is `publish_hero_artwork`'s,
    // for `MosaicGuard`'s reason.
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
        // A leave landing mid-post has rewound the count on its way out.
        if !fav_ui.section_active() {
            return;
        }
        ui.global::<Favorites>().set_track_count(count);
        // Order-free, the chips reading the handle rather than these properties.
        crate::ui::hero_chips::publish_favorites(&ui, &fav_ui);
    });
}
