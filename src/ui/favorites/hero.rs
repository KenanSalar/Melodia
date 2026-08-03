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

use slint::{ComponentHandle, Image, Model, SharedString, VecModel, Weak};

use super::FavoritesUi;
use crate::entities::track::FavoriteStats;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::mosaic_blur::{MosaicBlur, compose_mosaic_blur};
use crate::ui::now_playing::write_crossfade_slot;
use crate::{AppWindow, Favorites};

/// Fetch fresh stats, push the count + duration text + mosaic paths
/// into `Favorites`, then kick a blocking composition+blur task whose
/// result lands on the UI thread via `upgrade_in_event_loop`.
/// `animate` fades the cross-fade between the old mosaic and the new.
pub async fn refresh_hero(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    animate: bool,
) -> AppResult<()> {
    let stats = library::favorites::get_favorite_stats(state).await?;
    *fav_ui.state().stats.lock() = stats.clone();
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

fn push_stats_to_slint(
    stats: &FavoriteStats,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
) {
    let count = i32::try_from(stats.count).unwrap_or(i32::MAX);
    let paths = stats.artwork_paths.clone();
    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
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

fn clear_hero_blur(fav_ui: &Arc<FavoritesUi>, weak: &Weak<AppWindow>) {
    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !fav_ui.section_active() {
            return;
        }
        let g = ui.global::<Favorites>();
        // Same rule as `apply_hero_blur`: the guard records what is painted, so
        // it moves only past the check that decides whether anything is. A
        // clear dropped by that check leaves the previous covers recorded, and
        // the next refresh tries again.
        fav_ui.state().last_mosaic_paths.lock().clear();
        // With no mosaic left, the gradient floor is the whole backdrop —
        // re-solve against it so the scrim and foreground match what is
        // actually about to be on screen.
        crate::ui::hero_backdrop::reset(&ui);
        // `None` through write_crossfade_slot clears `has-blur` without
        // wiping the previous slot, so any in-flight fade-out completes
        // naturally before the gradient floor takes over.
        write_crossfade_slot(
            None,
            true,
            g.get_blur_use_a(),
            |img| g.set_blur_img_a(img),
            |img| g.set_blur_img_b(img),
            |v| g.set_blur_use_a(v),
            |v| g.set_has_blur(v),
        );
    });
}

/// Publish the composed mosaic, and record it as the one on screen. Skipped
/// outright once the section is no longer active: `HeroBackdrop` is shared by
/// all six heroes, so a compose that finishes after the user has navigated away
/// would paint this view's solve under whichever hero mounted next. Because the
/// `last_mosaic_paths` record lives past that check rather than before the
/// compose, a skip here leaves the next refresh free to try the same covers
/// again — the leave handler's `forget_mosaic` is a convenience, not the only
/// thing keeping the guard honest.
fn apply_hero_blur(
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    composed: Option<MosaicBlur>,
    animate: bool,
    paths: Vec<String>,
) {
    let fav_ui = fav_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !fav_ui.section_active() {
            return;
        }
        // Refreshes overlapping the compose window all get here with the same
        // covers, since none of them had recorded yet; the losers have nothing
        // to add but a redundant cross-fade of the same buffer. Rare on this
        // side — `refresh_hero` awaits its own `spawn_blocking`, and the
        // channel subscriber awaits `refresh_hero`, so ticks can't overlap
        // each other. What can race one is the section-enter fetch, which is
        // spawned detached: the first-enter kick at wire time, and every
        // re-enter after that.
        {
            let mut last = fav_ui.state().last_mosaic_paths.lock();
            if *last == paths {
                return;
            }
            *last = paths;
        }
        let g = ui.global::<Favorites>();
        // The hue and brightness were measured off this very buffer on the
        // blocking pool, so the scrim lands in step with the blur under it.
        let sample = composed.as_ref().map(|m| m.sample).unwrap_or_default();
        crate::ui::hero_backdrop::apply(&ui, sample);
        let img = composed.map(|m| Image::from_rgb8(m.blur));
        write_crossfade_slot(
            img,
            animate,
            g.get_blur_use_a(),
            |i| g.set_blur_img_a(i),
            |i| g.set_blur_img_b(i),
            |v| g.set_blur_use_a(v),
            |v| g.set_has_blur(v),
        );
    });
}

