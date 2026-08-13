//! Hero stats + live cover mosaic refresh.
//!
//! On every `library_changed_tx` tick the Favorites view is visible
//! for, this re-fetches `library::favorites::get_favorite_stats` and
//! produces a fresh hero blur from the top-4 most-played covers via the
//! shared [`crate::ui::mosaic_blur`] atlas+blur recipe. Write goes
//! through `write_crossfade_slot` so switching mosaics fades the
//! previous blur to the new one — outgoing slot stays painted for the
//! full fade so the hero never flashes empty.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString, VecModel, Weak};

use super::FavoritesUi;
use crate::entities::track::FavoriteStats;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::mosaic_blur::compose_mosaic_blur;
use crate::ui::mosaic_hero::impl_mosaic_hero;
use crate::{AppWindow, Favorites};

// The apply/clear pair is shared with the Recently-Played hero — same guard
// placement, same cross-fade, different global. Overlapping composes are rare
// on this side: `refresh_hero` awaits its own `spawn_blocking` and the channel
// subscriber awaits `refresh_hero`, so ticks can't overlap each other. What
// can race one is the section-enter fetch, which is spawned detached — the
// first-enter kick at wire time, and every re-enter after that.
impl_mosaic_hero!(Favorites, FavoritesUi);

/// Fetch fresh stats, push the count + mosaic paths into `Favorites` and the
/// band's chips with them, then kick a blocking composition+blur task whose
/// result lands on the UI thread via `upgrade_in_event_loop`.
/// `animate` fades the cross-fade between the old mosaic and the new.
///
/// The running time reaches the band as a chip rather than as a property: the
/// millisecond total is already on the stats struct, so routing a formatted
/// string through Slint only to read it back would be a round trip for
/// something this crate had in hand.
pub async fn refresh_hero(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    animate: bool,
) -> AppResult<()> {
    let stats = library::favorites::get_favorite_stats(state).await?;

    // A leave that landed while the query was in flight has already wiped
    // `stats` and emptied the mosaic-path model, so the store and the push
    // below would fill both back in behind a view nobody can see. The leave set
    // `mark_dirty`, so the next enter re-fetches — the guard `refresh_grids`
    // and `refresh_tracks` carry, in the same place, after the slow part.
    if !fav_ui.section_active() {
        return Ok(());
    }

    {
        // Under the section gate: `release_section_state` wipes this beside the
        // caches, so the two must not interleave. Synchronous store only.
        let _gate = fav_ui.gate();
        *fav_ui.state().stats.lock() = stats.clone();
    }
    push_stats_to_slint(&stats, fav_ui, weak);

    let paths = stats.artwork_paths.clone();
    if paths.is_empty() {
        clear_hero_blur(fav_ui, weak);
        return Ok(());
    }

    // Skip the decode+blur when the mosaic covers are already the ones on
    // screen — the blur is still correct, so a library/stats tick with the same
    // top-4 costs nothing. Reset on section-leave so a genuine re-enter
    // recomposes. The matching *record* is in `apply_hero_blur`, not here: the
    // guard means "this mosaic is what's painted", and recording a compose
    // whose apply is then dropped would wedge the hero on the gradient floor
    // for every later refresh of the same covers.
    if *fav_ui.state().last_mosaic_paths.lock() == paths {
        return Ok(());
    }

    // Composition, blur and the colour measurement are all CPU-bound — they
    // run on the blocking pool to keep the tokio worker free. What comes back
    // is a raw `SharedPixelBuffer` and its measurement, so it can cross the
    // `upgrade_in_event_loop` boundary (`slint::Image` is `!Send`, so the wrap
    // happens on the UI thread).
    let compose_paths = paths.clone();
    let composed = state
        .runtime
        .spawn_blocking(move || compose_mosaic_blur(&compose_paths))
        .await
        .ok()
        .flatten();

    apply_hero_blur(fav_ui, weak, composed, animate, paths);
    Ok(())
}

// The atlas composition itself lives in `crate::ui::mosaic_blur` — shared
// with the Recently-Played hero so both surfaces read identically.

fn push_stats_to_slint(stats: &FavoriteStats, fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let count = i32::try_from(stats.count).unwrap_or(i32::MAX);
    let paths = stats.artwork_paths.clone();
    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        // The leave can land while this post is in flight, and it empties the
        // mosaic-path model on its way out — the same guard, in the same place,
        // as `songs::apply_filtered_tracks`.
        if !fav_ui.section_active() {
            return;
        }
        let g = ui.global::<Favorites>();
        g.set_track_count(count);
        // Order-free: the chips take their facts off the handle's own state,
        // not back off the properties written around them.
        crate::ui::hero_chips::publish_favorites(&ui, &fav_ui);
        let model = g.get_mosaic_paths();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<SharedString>>() else {
            log::warn!("Favorites.mosaic-paths: VecModel<SharedString> downcast failed");
            return;
        };
        let rendered: Vec<SharedString> =
            paths.iter().map(|p| SharedString::from(p.as_str())).collect();
        vec.set_vec(rendered);
    });
}

/// Re-publish the band's chips on the UI thread.
///
/// **Call this wherever one of the chips' inputs lands.** Favorites is the one
/// hero built from three fetches rather than one: the stats, the Songs spread,
/// the Most Played totals — and `kick_full_refresh` runs those *concurrently*,
/// so no ordering can be assumed. Each fetch folds its own answer on its own
/// worker and then calls this; the publish itself reads only finished values,
/// so the worst a mistimed one can be is a tick behind, never half-built.
///
/// The grid path can't stand in for it: `write_filtered_grids` publishes past a
/// signature early-return, and `mounted_content` is a constant `0` on the Songs
/// tab — so on Songs that publish fires only when the column count moves.
pub fn republish_chips(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let fav_ui = fav_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        crate::ui::hero_chips::publish_favorites(&ui, &fav_ui);
    });
}
