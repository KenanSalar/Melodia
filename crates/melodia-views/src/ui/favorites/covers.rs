//! Which covers the Favorites view warms, and when.
//!
//! It owns no tier: both grid tabs draw from `ui::grid_prewarm::tier()` and the Songs tab's
//! `TrackList` from the shared row tier. What is left here is the projection — which paths, in
//! which order — and a tab pick's prewarm, which a fetch can await and a pick cannot.

use std::path::PathBuf;

use super::state::GRID_PREWARM_AHEAD;
use super::{FavoritesTab, FavoritesUi};

impl FavoritesUi {
    /// First-screenful cover paths for a grid tab, in display order.
    ///
    /// Deduped and capped by the shared [`crate::ui::grid_prewarm`] helper — the cap bounds *kept
    /// paths*, not input items, so an uncapped grid over a large library doesn't allocate a
    /// `PathBuf` per unique cover just to keep the first two rows.
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

    /// Decode a grid tab's first screenful into the grid tier. Blocking — call it from
    /// `spawn_blocking`, never on the UI thread.
    ///
    /// Songs draws no cards, so it warms nothing. No section re-check on the way out: the tier
    /// outlives the leave now, so buffers decoded for a view the user has left are the ones that
    /// paint when they come back to it.
    ///
    /// **`GRID_PREWARM_AHEAD`, not the tier capacity** — the grids are uncapped, so warming
    /// everything evicts its own work.
    pub fn prewarm_tab_covers(&self, tab: FavoritesTab) {
        let paths = self.first_screenful_paths(tab);
        if !paths.is_empty() {
            crate::ui::grid_prewarm::prewarm(&paths);
        }
    }
}
