//! Which covers the Recently-Played view warms, and when.
//!
//! It owns no tier: the Most Played cards draw from `ui::grid_prewarm::tier()` and the Songs tab's
//! `TrackList` from the shared row tier. What is left here is the projection — which paths, in
//! which order — and a tab pick's prewarm, which a fetch can await and a pick cannot.

use std::path::PathBuf;

use super::state::GRID_PREWARM_AHEAD;
use super::{RecentlyPlayedTab, RecentlyPlayedUi};

impl RecentlyPlayedUi {
    /// First-screenful cover paths for the Most Played tab, in display order.
    ///
    /// Deduped and capped by the shared [`crate::ui::grid_prewarm`] helper —
    /// the cap bounds *kept paths*, not input items, so an uncapped grid over
    /// a large library doesn't allocate a `PathBuf` per unique cover just to
    /// keep the first two rows.
    pub fn first_screenful_paths(&self, tab: RecentlyPlayedTab) -> Vec<PathBuf> {
        match tab {
            RecentlyPlayedTab::MostPlayed => crate::ui::grid_prewarm::unique_artwork_paths(
                self.state().most_played.lock().iter().map(|t| t.artwork_path.as_deref()),
                GRID_PREWARM_AHEAD,
            ),
            RecentlyPlayedTab::Songs => Vec::new(),
        }
    }

    /// Decode a tab's first screenful into the grid tier. Blocking — call it from
    /// `spawn_blocking`, never on the UI thread.
    ///
    /// Songs draws no cards, so it warms nothing. No section re-check on the way out: the tier
    /// outlives the leave now, so buffers decoded for a view the user has left are the ones that
    /// paint when they come back to it.
    pub fn prewarm_tab_covers(&self, tab: RecentlyPlayedTab) {
        let paths = self.first_screenful_paths(tab);
        if !paths.is_empty() {
            crate::ui::grid_prewarm::prewarm(&paths);
        }
    }
}
