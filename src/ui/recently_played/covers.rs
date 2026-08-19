//! The Recently-Played view's two cover tiers: what each holds, when it warms,
//! and when it is handed back.
//!
//! One is the view's own — Most Played, at grid size — and the other is the
//! shared row tier the Songs tab's `TrackList` draws from, which this module
//! only ever reads. Tab-leave is a release event as much as a section leave is,
//! so the grid tier is dropped and re-warmed on a tab pick too.

use std::path::PathBuf;

use slint::Image;

use super::state::{GRID_PREWARM_AHEAD, GRID_THUMB_CAP};
use super::{RecentlyPlayedTab, RecentlyPlayedUi};
use crate::AppWindow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::grid_prewarm::grid_cover;

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

    /// The cover tier a tab draws from. `None` for Songs, whose row covers come
    /// from the shared row tier instead.
    fn grid_tier(&self, tab: RecentlyPlayedTab) -> Option<&CoverThumbs> {
        match tab {
            RecentlyPlayedTab::MostPlayed => Some(&self.most_played_thumbs),
            RecentlyPlayedTab::Songs => None,
        }
    }

    /// Decode a tab's first screenful into its tier. Blocking — call it from
    /// `spawn_blocking`, never on the UI thread.
    ///
    /// Hands the buffers straight back when the section was left while the
    /// decode ran. [`RecentlyPlayedUi::release_section_state`] is spawned on that
    /// leave and may well have finished first, so without this the tier it
    /// emptied comes back populated behind a view nobody can see and stays that
    /// way until the next leave. The check has to sit *after* the decode — before
    /// it, the leave hasn't happened yet, which is the whole problem.
    ///
    /// Returns whether the tier is warm on the way out — `false` when there was
    /// nothing to decode, and when the buffers went straight back. Callers
    /// announce a warm tier to the cards by bumping
    /// `RecentlyPlayed.covers-generation`, and announcing one this returned
    /// `false` for is worse than not announcing at all: it puts every mounted
    /// card back on the decode-on-miss path, on the UI thread, which is the state
    /// the counter exists to prevent.
    pub fn prewarm_tab_covers(&self, tab: RecentlyPlayedTab) -> bool {
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

    /// Hand the grid tier over to `entering`: drop whatever the tab being left
    /// was holding, then decode the new tab's first screenful. Blocking — call it
    /// from `spawn_blocking`, never on the UI thread.
    ///
    /// Releasing first is what keeps the peak at one tier rather than two; the
    /// single `heap_trim` comes last, after the prewarm has taken the pages it
    /// needs, so we don't hand them back only to ask for them again. Songs holds
    /// no tier of its own, so entering it releases and warms nothing.
    ///
    /// Bails when a later pick has already overtaken this one, the way
    /// [`RecentlyPlayedUi::release_section_state`] bails on a re-enter: two of
    /// these racing on the blocking pool would otherwise let the loser clear the
    /// tier the winner just warmed.
    ///
    /// Returns [`RecentlyPlayedUi::prewarm_tab_covers`]' verdict, which the caller
    /// owes the same respect: a bail here, or a leave landing mid-decode, means
    /// there is no warm tier to announce.
    pub fn swap_tab_covers(&self, entering: RecentlyPlayedTab) -> bool {
        if self.active_tab() != entering {
            return false;
        }
        if entering != RecentlyPlayedTab::MostPlayed {
            self.most_played_thumbs.clear();
        }
        let warm = self.prewarm_tab_covers(entering);
        crate::tasks::heap_trim::trim();
        warm
    }

    /// Lazy cover lookup for the Most Played grid cards. Routed via
    /// `RecentlyPlayed.request-most-played-cover`.
    pub fn most_played_cover(&self, artwork_path: &str, generation: i32) -> Image {
        grid_cover(&self.most_played_thumbs, artwork_path, generation)
    }
}

/// Retune the Most Played cover cache to the real display resolution. Called after
/// `app.show()` and again on every resize, alongside the entity grids' own
/// tuning — the tab draws the same card at the same size, so it takes the
/// same band.
pub fn tune_cache_for_display(app: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    let cap = crate::ui::grid_prewarm::cover_cap_for_window(app, GRID_THUMB_CAP);
    let size = crate::ui::grid_prewarm::cover_size_for_window(app);
    rp_ui.most_played_thumbs.resize(cap);
    rp_ui.most_played_thumbs.set_thumb_size(size);
    log::debug!("ui::recently_played grid-cover cache tuned to cap {cap}, {size} px");
}
