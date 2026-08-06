//! The Favorites view's four cover tiers: what each holds, when it warms,
//! and when it is handed back.
//!
//! Three of them are the view's own — the hero mosaic (128 px), Most Played
//! and Favorite Artists (grid-sized) — and the fourth is the shared 72 px row
//! tier the Songs tab's `TrackList` draws from, which this module only ever
//! reads. Tab-leave is the event the collapse toggles used to be, so the two
//! grid tiers are released and re-warmed on a tab pick as well as on a
//! section leave.

use std::path::PathBuf;

use slint::Image;

use super::state::{GRID_PREWARM_AHEAD, GRID_THUMB_CAP};
use super::{FavoritesTab, FavoritesUi};
use crate::AppWindow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::grid_prewarm::{grid_cover, nonempty_artwork_path};

impl FavoritesUi {
    /// First-screenful cover paths for a grid tab, in display order.
    ///
    /// Deduped and capped by the shared [`crate::ui::grid_prewarm`] helper —
    /// the cap bounds *kept paths*, not input items, so an uncapped grid over
    /// a large library doesn't allocate a `PathBuf` per unique cover just to
    /// keep the first two rows.
    pub fn first_screenful_paths(&self, tab: FavoritesTab) -> Vec<PathBuf> {
        match tab {
            FavoritesTab::MostPlayed => crate::ui::grid_prewarm::unique_artwork_paths(
                self.state().most_played.lock().iter().map(|t| t.artwork_path.as_deref()),
                GRID_PREWARM_AHEAD,
            ),
            FavoritesTab::Artists => crate::ui::grid_prewarm::unique_artwork_paths(
                self.state().fav_artists.lock().iter().map(|a| a.image_path.as_deref()),
                GRID_PREWARM_AHEAD,
            ),
            FavoritesTab::Songs => Vec::new(),
        }
    }

    /// The cover tier a grid tab draws from. `None` for Songs, whose row
    /// covers come from the shared 72 px tier instead.
    fn grid_tier(&self, tab: FavoritesTab) -> Option<&CoverThumbs> {
        match tab {
            FavoritesTab::MostPlayed => Some(&self.most_played_thumbs),
            FavoritesTab::Artists => Some(&self.artist_thumbs),
            FavoritesTab::Songs => None,
        }
    }

    /// Decode a grid tab's first screenful into its tier. Blocking — call it
    /// from `spawn_blocking`, never on the UI thread.
    ///
    /// Hands the buffers straight back when the section was left while the
    /// decode ran. [`FavoritesUi::release_section_state`] is spawned on that
    /// leave and may well have finished first, so without this the tier it
    /// emptied comes back populated behind a view nobody can see and stays
    /// that way until the next leave. The check has to sit *after* the decode
    /// — before it, the leave hasn't happened yet, which is the whole problem.
    ///
    /// Returns whether the tier is warm on the way out — `false` when there was
    /// nothing to decode, and when the buffers went straight back. Callers
    /// announce a warm tier to the cards by bumping `Favorites.covers-generation`,
    /// and announcing one this returned `false` for is worse than not announcing
    /// at all: it puts every mounted card back on the decode-on-miss path, on
    /// the UI thread, which is the state the counter exists to prevent.
    pub fn prewarm_tab_covers(&self, tab: FavoritesTab) -> bool {
        let Some(thumbs) = self.grid_tier(tab) else {
            return false;
        };
        let paths = self.first_screenful_paths(tab);
        if paths.is_empty() {
            return false;
        }
        thumbs.prewarm(&paths);
        if !self.section_active() {
            thumbs.clear();
            return false;
        }
        true
    }

    /// Hand the grid tiers over to `entering`: drop whatever the tab being
    /// left was holding, then decode the new tab's first screenful. Blocking —
    /// call it from `spawn_blocking`, never on the UI thread.
    ///
    /// Releasing first is what keeps the peak at one tier rather than two; the
    /// single `heap_trim` comes last, after the prewarm has taken the pages it
    /// needs, so we don't hand them back only to ask for them again. Songs
    /// holds neither tier, so entering it releases both and warms nothing.
    ///
    /// Bails when a later pick has already overtaken this one, the way
    /// [`FavoritesUi::release_section_state`] bails on a re-enter: two of these
    /// racing on the blocking pool would otherwise let the loser clear the tier
    /// the winner just warmed.
    ///
    /// Returns [`FavoritesUi::prewarm_tab_covers`]' verdict, which the caller
    /// owes the same respect: a bail here, or a leave landing mid-decode, means
    /// there is no warm tier to announce.
    pub fn swap_tab_covers(&self, entering: FavoritesTab) -> bool {
        if self.active_tab() != entering {
            return false;
        }
        if entering != FavoritesTab::MostPlayed {
            self.most_played_thumbs.clear();
        }
        if entering != FavoritesTab::Artists {
            self.artist_thumbs.clear();
        }
        let warm = self.prewarm_tab_covers(entering);
        crate::tasks::heap_trim::trim();
        warm
    }

    /// Drop just the Favorite Artists grid's cover cache. Called (off the UI
    /// thread) when a card drills into Artist Detail: `Nav` flips, the grid is
    /// unmounted, and its covers are no longer visible or queried via
    /// `request-artist-cover`. Mirrors
    /// [`crate::ui::albums::AlbumsUi::release_grid_covers`], which does the
    /// same on drill-in.
    pub fn release_artist_covers(&self) {
        self.artist_thumbs.clear();
        crate::tasks::heap_trim::trim();
    }

    /// Lazy cover lookup for the hero 2x2 mosaic tiles. Routed via
    /// `Favorites.request-mosaic-cover`.
    pub fn mosaic_cover(&self, artwork_path: &str) -> Image {
        self.mosaic_thumbs
            .get_or_load_opt(nonempty_artwork_path(artwork_path))
    }

    /// Lazy cover lookup for the Most Played grid cards. Routed via
    /// `Favorites.request-most-played-cover`.
    pub fn most_played_cover(&self, artwork_path: &str, generation: i32) -> Image {
        grid_cover(&self.most_played_thumbs, artwork_path, generation)
    }

    /// Lazy cover lookup for the Favorite Artists circular cards.
    pub fn artist_cover(&self, artwork_path: &str, generation: i32) -> Image {
        grid_cover(&self.artist_thumbs, artwork_path, generation)
    }
}

/// Retune both grid-tier cover caches to the real display resolution. Called
/// once at startup after the winit window is live, alongside the entity
/// grids' own tuning — the tabs draw the same card at the same size, so they
/// take the same band.
///
/// Both, even though only one is ever warm: which tab the user resumes on
/// isn't known until [`super::seed_tab`], and resizing an empty LRU costs
/// nothing.
pub fn tune_cache_for_display(app: &AppWindow, fav_ui: &FavoritesUi) {
    let cap = crate::ui::grid_prewarm::cover_cap_for_window(app, GRID_THUMB_CAP);
    fav_ui.most_played_thumbs.resize(cap);
    fav_ui.artist_thumbs.resize(cap);
    log::debug!("ui::favorites grid-cover cache caps tuned to {cap}");
}
