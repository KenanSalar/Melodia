//! The band's stats and collage, both taken off the recency rows rather than a query of their
//! own; the collage is the latest four distinct covers, composed through
//! [`crate::ui::mosaic_hero`].

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::RecentlyPlayedUi;
use crate::ui::detail_view::impl_curated_hero_helpers;
use melodia_app::state::AppState;
use melodia_core::entities::track::TrackListRow as RsTrackListRow;
use melodia_ui::{AppWindow, RecentlyPlayed};

impl_curated_hero_helpers!(RecentlyPlayed, RecentlyPlayedUi);

/// The first `n` distinct covers in recency order, as the strings the compose takes.
pub fn mosaic_paths_from(rows: &[RsTrackListRow], n: usize) -> Vec<String> {
    crate::ui::grid_prewarm::unique_artwork_paths(rows.iter().map(|r| r.artwork_path.as_deref()), n)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Publishes the count and the band's chips, which read the facts `songs::refresh_tracks` stored
/// before calling in.
pub fn push_hero_stats(count: i32, rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        // A leave landing mid-post has rewound the count on its way out.
        if !rp_ui.section_active() {
            return;
        }
        ui.global::<RecentlyPlayed>().set_track_count(count);
        // Order-free, the chips reading the handle rather than these properties.
        crate::ui::hero_chips::publish_recently_played(&ui, &rp_ui);
    });
}

/// Composes and publishes the banner; an empty list clears it to the gradient floor. Spawned
/// detached, so the next tick can start a second compose before this one claims the guard, and
/// `MosaicGuard::claim` is what makes the second publish a no-op.
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
