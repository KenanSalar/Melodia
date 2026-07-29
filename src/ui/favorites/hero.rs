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

use slint::{ComponentHandle, Image, Model, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel,
    Weak};

use super::FavoritesUi;
use crate::entities::track::FavoriteStats;
use crate::error::AppResult;
use crate::library;
use crate::state::AppState;
use crate::ui::mosaic_blur::compose_mosaic_blur;
use crate::ui::now_playing::write_crossfade_slot;
use crate::{AppWindow, Favorites};

/// Fetch fresh stats, push the count + duration text + mosaic paths
/// into `Favorites`, then kick a blocking composition+blur task whose
/// result lands on the UI thread via `upgrade_in_event_loop`. The
/// blur step is gated by `animate` — `true` for live refreshes (the
/// user is looking at the page), `false` for the seed-on-section-enter
/// case where we want the new state already in place when the view
/// becomes visible.
pub async fn refresh_hero(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &Weak<AppWindow>,
    animate: bool,
) -> AppResult<()> {
    let stats = library::favorites::get_favorite_stats(state).await?;
    *fav_ui.state().stats.lock() = stats.clone();
    push_stats_to_slint(&stats, weak);

    let paths = stats.artwork_paths.clone();
    if paths.is_empty() {
        // Reset the guard so the next non-empty refresh (even with covers
        // identical to a pre-empty state) recomposes.
        fav_ui.state().last_mosaic_paths.lock().clear();
        clear_hero_blur(weak);
        return Ok(());
    }

    // Skip the decode+blur when the mosaic covers are unchanged from the last
    // composed set — the blur already on screen is still correct, so a
    // library/stats tick with the same top-4 covers costs nothing. Reset on
    // section-leave so a genuine re-enter recomposes.
    {
        let mut last = fav_ui.state().last_mosaic_paths.lock();
        if *last == paths {
            return Ok(());
        }
        last.clone_from(&paths);
    }

    // Composition + blur is CPU-bound — runs on the blocking pool to
    // keep the tokio worker free. Returns the raw `SharedPixelBuffer`
    // so it can cross the `upgrade_in_event_loop` boundary (`slint::
    // Image` is `!Send` so the wrap happens on the UI thread).
    let blur_buf = state
        .runtime
        .spawn_blocking(move || compose_mosaic_blur(&paths))
        .await
        .ok()
        .flatten();

    apply_hero_blur(weak, blur_buf, animate);
    Ok(())
}

// The atlas composition itself lives in `crate::ui::mosaic_blur` — shared
// with the Recently-Played hero so both surfaces read identically.

fn push_stats_to_slint(stats: &FavoriteStats, weak: &Weak<AppWindow>) {
    let count = i32::try_from(stats.count).unwrap_or(i32::MAX);
    let duration = crate::ui::tracks::format_duration_ms(stats.total_duration_ms);
    let paths = stats.artwork_paths.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        g.set_track_count(count);
        g.set_duration_text(SharedString::from(duration));
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

fn clear_hero_blur(weak: &Weak<AppWindow>) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
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

fn apply_hero_blur(
    weak: &Weak<AppWindow>,
    buf: Option<SharedPixelBuffer<Rgb8Pixel>>,
    animate: bool,
) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<Favorites>();
        // Measure before the buffer is consumed by the `Image` wrap.
        crate::ui::hero_backdrop::apply(&ui, buf.as_ref());
        let img = buf.map(Image::from_rgb8);
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

