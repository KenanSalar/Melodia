//! Hero stats + live cover mosaic/blur for the Recently-Played view.
//!
//! Unlike the Favorites hero (which re-queries `get_favorite_stats`), the data
//! here is derived from the already-fetched recency rows: the mosaic is the
//! up-to-4 most-recently-played *distinct* covers, and the count/duration are
//! summed off the same rows. The blur backdrop reuses the shared dual-slot
//! cross-fade (`write_crossfade_slot`) and the shared
//! [`crate::ui::mosaic_blur`] atlas+blur recipe so both hero surfaces read
//! identically.

use std::collections::HashSet;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, SharedString, VecModel, Weak};

use super::RecentlyPlayedUi;
use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::state::AppState;
use crate::ui::backdrop::BackdropSample;
use crate::ui::mosaic_blur::{MosaicBlur, compose_mosaic_blur};
use crate::ui::now_playing::write_crossfade_slot;
use crate::ui::tracks::format_duration_ms;
use crate::{AppWindow, RecentlyPlayed};

/// The up-to-`n` most-recently-played *distinct* cover paths, in recency
/// order — the hero mosaic tiles. Skips empty/absent artwork.
pub fn mosaic_paths_from(rows: &[RsTrackListRow], n: usize) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(n);
    for r in rows {
        let Some(p) = r.artwork_path.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        if seen.insert(p) {
            out.push(p.to_owned());
            if out.len() == n {
                break;
            }
        }
    }
    out
}

/// Push the hero count + total-duration text + mosaic-path list into the Slint
/// global. Immediate (the blur composition is kicked separately).
pub fn push_hero_stats(count: i32, total_ms: i64, mosaic_paths: &[String], weak: &Weak<AppWindow>) {
    let duration = format_duration_ms(total_ms);
    let paths = mosaic_paths.to_vec();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<RecentlyPlayed>();
        g.set_track_count(count);
        g.set_duration_text(SharedString::from(duration.as_str()));
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
    let composed = state
        .runtime
        .spawn_blocking(move || compose_mosaic_blur(&mosaic_paths))
        .await
        .ok()
        .flatten();
    apply_hero_blur(rp_ui, weak, composed, animate);
}

/// Clear the hero blur (e.g. no covers) without wiping the previous slot, so an
/// in-flight fade completes before the gradient floor takes over.
pub fn clear_hero_blur(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !rp_ui.section_active() {
            return;
        }
        let g = ui.global::<RecentlyPlayed>();
        // With no mosaic left, the gradient floor is the whole backdrop —
        // re-solve against it so the scrim and foreground match what is
        // actually about to be on screen.
        crate::ui::hero_backdrop::reset(&ui);
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

/// Publish the composed mosaic. Skipped outright once the section is no longer
/// active: `HeroBackdrop` is shared by all six heroes, so a compose that
/// finishes after the user has navigated away would paint this view's solve
/// under whichever hero mounted next. The leave handler calls
/// `forget_mosaic`, so a genuine re-enter recomposes.
fn apply_hero_blur(
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
    composed: Option<MosaicBlur>,
    animate: bool,
) {
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !rp_ui.section_active() {
            return;
        }
        let g = ui.global::<RecentlyPlayed>();
        // The hue and brightness were measured off this very buffer on the
        // blocking pool, so the scrim lands in step with the blur under it.
        let (img, sample) = match composed {
            Some(m) => (Some(Image::from_rgb8(m.blur)), m.sample),
            None => (None, BackdropSample::default()),
        };
        crate::ui::hero_backdrop::apply(&ui, sample);
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

// The atlas composition itself lives in `crate::ui::mosaic_blur` — shared
// with the Favorites hero so both surfaces read identically.
