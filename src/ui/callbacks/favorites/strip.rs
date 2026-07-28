//! `Favorites.*` strip-card actions: Most Played play-track, Favorite
//! Artists play-artist, the cross-tab open-artist hand-off, and the two
//! sub-section collapse toggles. See [`super::wire_favorites`].

use std::sync::Arc;

use slint::ComponentHandle;

use super::NAV_FAVORITES;
use crate::library;
use crate::state::AppState;
use crate::ui::artists::ArtistsUi;
use crate::ui::callbacks::cross_tab_nav;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::favorites::FavoritesUi;
use crate::{AppWindow, Favorites};

/// Wire the strip-card + collapse-toggle callbacks.
pub(super) fn wire(
    ui: &AppWindow,
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    artists_ui: &Arc<ArtistsUi>,
) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // play-track: clicking a Most Played card loads the strip into the queue
    // and starts on that card — the strip is the context, not the All Songs
    // list below it. The Slint callback carries no row index (these are cards,
    // not list rows), so the start slot comes from the id.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        g.on_play_track(move |id| {
            let id = i64::from(id);
            let ids = fu.most_played_track_ids();
            if ids.is_empty() {
                return;
            }
            let start = ids.iter().position(|&i| i == id);
            let s = s.clone();
            spawn_logged!(s, "favorites::play_track",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }
    {
        let s = state.clone();
        g.on_play_artist(move |id| {
            let s = s.clone();
            let id = i64::from(id);
            s.runtime.clone().spawn(async move {
                let tracks = match library::artists::get_artist_tracks(&s, id).await {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!("favorites::play_artist fetch: {e}");
                        return;
                    }
                };
                let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
                if ids.is_empty() {
                    return;
                }
                if let Err(e) =
                    library::playback::player_play_tracks(&s.playback_ctx(), ids, Some(0)).await
                {
                    log::warn!("favorites::play_artist: {e}");
                }
            });
        });
    }

    // --- Cross-tab open-artist ------------------------------------
    // Clicking a favorite artist card drills into the Artists tab's
    // Artist Detail; the shared hand-off stamps the origin so the back
    // arrow returns to Favorites.
    {
        let s = state.clone();
        let aru = artists_ui.clone();
        let weak = weak.clone();
        g.on_open_artist(move |artist_id| {
            cross_tab_nav::open_artist_cross_tab(
                &s,
                &aru,
                &weak,
                i64::from(artist_id),
                NAV_FAVORITES,
                "favorites::open_artist",
            );
        });
    }

    // --- Favorite Artists collapse toggle ------------------------
    // Flip `Favorites.artists-collapsed`, then persist to
    // `views.json`'s `favorites_artists_collapsed` so the next launch
    // re-opens the sub-section in the same state. Mirrors
    // `ArtistDetail.toggle-albums-collapsed`. Collapsing also drops the
    // strip's 200 px cover LRU (the `if !artists-collapsed` gate has
    // unmounted the scroller, so the covers are no longer visible) —
    // re-expanding re-decodes them lazily on card mount.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_toggle_artists_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Favorites>();
            let next = !g.get_artists_collapsed();
            g.set_artists_collapsed(next);
            let s_disk = s.clone();
            let fu = fu.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_favorites_artists_collapsed(&s_disk, next)
                {
                    log::warn!("favorites::set_favorites_artists_collapsed: {e}");
                }
                if next {
                    fu.release_artist_covers();
                }
            });
        });
    }

    // --- Most Played collapse toggle ------------------------------
    // Same shape as the artists toggle above — collapsing drops the
    // 180 px Most Played cover LRU.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        g.on_toggle_most_played_collapsed(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Favorites>();
            let next = !g.get_most_played_collapsed();
            g.set_most_played_collapsed(next);
            let s_disk = s.clone();
            let fu = fu.clone();
            s.runtime.spawn_blocking(move || {
                if let Err(e) =
                    library::settings::set_favorites_most_played_collapsed(&s_disk, next)
                {
                    log::warn!("favorites::set_favorites_most_played_collapsed: {e}");
                }
                if next {
                    fu.release_most_played_covers();
                }
            });
        });
    }
}
