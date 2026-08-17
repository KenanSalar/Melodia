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
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::state::AppState;
use crate::ui::detail_artwork::DetailPair;
use crate::ui::detail_view::impl_detail_view_helpers;
use crate::{AppWindow, RecentlyPlayed};

// Only the artwork half — this page's track model is its own tabbed cache's.
impl_detail_view_helpers!(artwork_only RecentlyPlayed);

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

/// Re-publish the band's chips on the UI thread.
///
/// **Call this wherever one of the chips' inputs lands.** `kick_full_refresh`
/// runs the recency fetch and the Most Played fetch *concurrently*, and the band
/// states something from each depending on which tab is mounted — so no ordering
/// can be assumed. Each fetch folds its own answer on its own worker and then
/// calls this; the publish itself reads only finished values, so the worst a
/// mistimed one can be is a tick behind, never half-built.
///
/// The grid path can't stand in for it: `write_filtered_grid` publishes past a
/// signature early-return, and `mounted_content` is a constant `0` on the Songs
/// tab — so there that publish fires only when the column count moves.
pub fn republish_chips(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let rp_ui = rp_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::hero_chips::publish_recently_played(&ui, &rp_ui);
    });
}

/// Compose the banner artwork from `mosaic_paths` and publish it. The CPU-bound compose,
/// blur and colour measurement run on the blocking pool; the result lands on the UI
/// thread. `animate` fades the cross-fade (true for live refreshes). An empty list
/// composes to an empty pair, which clears the banner back to the gradient floor.
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

/// Publish a composed banner and claim it as the one on screen.
///
/// **Gated whole, where a detail view fills its own slots even while hidden.** This page's
/// leave wipes its models and forgets the guard, so slots written behind it have nothing to
/// be ready for and their claim would suppress the re-enter's recompose. What the gate
/// mainly protects is still `HeroBackdrop`, shared by all six heroes: a compose finishing
/// after a nav away would paint this page's solve under whichever hero mounted next.
///
/// Overlapping composes are more reachable here than on Favorites — `refresh_tracks` spawns
/// this detached, so the subscriber loop can come round again before the guard is claimed,
/// once per tick. `MosaicGuard::claim` is what makes the second one a no-op.
fn publish_hero_artwork(
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
    pair: DetailPair,
    animate: bool,
    paths: Vec<String>,
) {
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !rp_ui.section_active() || !rp_ui.state().last_mosaic_paths.claim(paths) {
            return;
        }
        apply_detail_artwork(&ui, &ui.global::<RecentlyPlayed>(), pair, animate, true);
    });
}
