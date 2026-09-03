//! Hero stats + the banner artwork for the Recently-Played view.
//!
//! Unlike the Favorites hero (which re-queries `get_favorite_stats`), the data
//! here is derived from the already-fetched recency rows: the collage is the
//! up-to-4 most-recently-played *distinct* covers composed into one image by
//! [`crate::ui::mosaic_hero`], and the count/duration are summed off the same
//! rows. Past the compose the banner is an ordinary single-artwork hero.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::RecentlyPlayedUi;
use crate::ui::detail_view::impl_detail_view_helpers;
use crate::{AppWindow, RecentlyPlayed};
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;

// Only the artwork half — this page's track model is its own tabbed cache's.
impl_detail_view_helpers!(
    curated RecentlyPlayed,
    RecentlyPlayedUi,
    crate::ui::hero_chips::publish_recently_played
);

/// The up-to-`n` most-recently-played *distinct* cover paths, in recency
/// order — the collage's sources. Skips empty/absent artwork.
///
/// The dedup is [`crate::ui::grid_prewarm::unique_artwork_paths`]', the same
/// one every cover prewarm in the tree uses; only the `String` shape the
/// Slint model wants is this function's own.
pub fn mosaic_paths_from(rows: &[RsTrackListRow], n: usize) -> Vec<String> {
    crate::ui::grid_prewarm::unique_artwork_paths(rows.iter().map(|r| r.artwork_path.as_deref()), n)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Push the hero count into the Slint global, and the band's chips with it.
/// Immediate (the collage is composed separately).
///
/// The running time and the spread reach the band as chips off the handle's own
/// state, which `songs::refresh_tracks` filled before calling in. Routing a
/// formatted string through Slint only to read it back would be a round trip for
/// something this crate had in hand — and the band is per-tab now, so the facts
/// have to outlive the fetch that folded them anyway.
pub fn push_hero_stats(count: i32, rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        // The leave can land while this post is in flight, and it rewinds the count on its
        // way out — the same guard, in the same place, as `songs::apply_filtered_tracks`.
        if !rp_ui.section_active() {
            return;
        }
        ui.global::<RecentlyPlayed>().set_track_count(count);
        // Order-free: the chips take their facts off the handle's own state, not
        // back off the properties written around them.
        crate::ui::hero_chips::publish_recently_played(&ui, &rp_ui);
    });
}

/// Compose the banner artwork from `mosaic_paths` and publish it. The CPU-bound compose,
/// blur and colour measurement run on the blocking pool; the result lands on the UI
/// thread. `animate` fades the cross-fade (true for live refreshes). An empty list
/// composes to an empty pair, which clears the banner back to the gradient floor.
///
/// Overlapping composes are more reachable here than on Favorites — `refresh_tracks` spawns
/// this detached, so the subscriber loop can come round again before the guard is claimed,
/// once per tick. `MosaicGuard::claim` is what makes the second one a no-op.
pub async fn refresh_artwork(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    mosaic_paths: Vec<String>,
    weak: &Weak<AppWindow>,
    animate: bool,
) {
    let Some(pair) =
        crate::ui::mosaic_hero::compose_off_thread(state, mosaic_paths.clone(), rp_ui.hero_blur)
            .await
    else {
        return;
    };
    publish_hero_artwork(rp_ui, weak, pair, animate, mosaic_paths);
}
