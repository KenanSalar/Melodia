//! Hero stats + live cover mosaic/blur for the Recently-Played view.
//!
//! Unlike the Favorites hero (which re-queries `get_favorite_stats`), the data
//! here is derived from the already-fetched recency rows: the mosaic is the
//! up-to-4 most-recently-played *distinct* covers, and the count/duration are
//! summed off the same rows. The blur backdrop reuses the shared dual-slot
//! cross-fade (`write_crossfade_slot`) and the shared
//! [`crate::ui::mosaic_blur`] atlas+blur recipe so both hero surfaces read
//! identically.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::RecentlyPlayedUi;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::state::AppState;
use crate::ui::mosaic_blur::compose_mosaic_blur;
use crate::ui::mosaic_hero::impl_mosaic_hero;
use crate::{AppWindow, RecentlyPlayed};

// The apply/clear pair is shared with the Favorites hero — same guard
// placement, same cross-fade, different global. Overlapping composes are more
// reachable here: `refresh_tracks` spawns the compose *detached*, so the
// subscriber loop is free to come round again and re-read a guard nobody has
// written, once per tick. Bounded in practice by the `get_recently_played` +
// full-capacity cover prewarm every tick pays first.
impl_mosaic_hero!(RecentlyPlayed, RecentlyPlayedUi);

/// The up-to-`n` most-recently-played *distinct* cover paths, in recency
/// order — the hero mosaic tiles. Skips empty/absent artwork.
///
/// The dedup is [`crate::ui::grid_prewarm::unique_artwork_paths`]', the same
/// one every cover prewarm in the tree uses; only the `String` shape the
/// Slint model wants is this function's own.
pub fn mosaic_paths_from(rows: &[RsTrackListRow], n: usize) -> Vec<String> {
    crate::ui::grid_prewarm::unique_artwork_paths(
        rows.iter().map(|r| r.artwork_path.as_deref()),
        n,
    )
    .into_iter()
    .map(|p| p.to_string_lossy().into_owned())
    .collect()
}

/// Push the hero count + mosaic-path list into the Slint global, and the band's
/// chips with them. Immediate (the blur composition is kicked separately).
///
/// The running time and the spread reach the band as chips off the handle's own
/// state, which `songs::refresh_tracks` filled before calling in. Routing a
/// formatted string through Slint only to read it back would be a round trip for
/// something this crate had in hand — and the band is per-tab now, so the facts
/// have to outlive the fetch that folded them anyway.
pub fn push_hero_stats(
    count: i32,
    mosaic_paths: &[String],
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
) {
    let paths = mosaic_paths.to_vec();
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        // The leave can land while this post is in flight, and it empties the
        // mosaic-path model on its way out — the same guard, in the same place,
        // as `songs::apply_filtered_tracks`.
        if !rp_ui.section_active() {
            return;
        }
        let g = ui.global::<RecentlyPlayed>();
        g.set_track_count(count);
        // Order-free: the chips take their facts off the handle's own state, not
        // back off the properties written around them.
        crate::ui::hero_chips::publish_recently_played(&ui, &rp_ui);
        let model = g.get_mosaic_paths();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<SharedString>>() else {
            log::warn!("RecentlyPlayed.mosaic-paths: VecModel<SharedString> downcast failed");
            return;
        };
        let rendered: Vec<SharedString> =
            paths.iter().map(|p| SharedString::from(p.as_str())).collect();
        vec.set_vec(rendered);
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

/// Compose + apply the hero blur from `mosaic_paths` (or clear it when empty).
/// The CPU-bound composition and colour measurement run on the blocking pool;
/// the result lands on the UI thread. `animate` fades the cross-fade (true for
/// live refreshes).
pub async fn refresh_blur(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    mosaic_paths: Vec<String>,
    weak: &Weak<AppWindow>,
    animate: bool,
) {
    if mosaic_paths.is_empty() {
        clear_hero_blur(rp_ui, weak);
        return;
    }
    let compose_paths = mosaic_paths.clone();
    let composed = state
        .runtime
        .spawn_blocking(move || compose_mosaic_blur(&compose_paths))
        .await
        .ok()
        .flatten();
    apply_hero_blur(rp_ui, weak, composed, animate, mosaic_paths);
}

// The atlas composition itself lives in `crate::ui::mosaic_blur`, and its
// application in `crate::ui::mosaic_hero` — both shared with the Favorites
// hero so the two surfaces read identically.
