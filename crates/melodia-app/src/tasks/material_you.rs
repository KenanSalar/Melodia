//! Material You coordinator: derives a palette from the playing cover and hands it to the UI
//! thread to paint.
//!
//! The appearance settings are re-read only on a kick, every write to them kicking after it
//! persists, so a view-model wake (the per-emit playback path) never touches the filesystem. Both
//! inputs are `watch` receivers, so a burst of clicks coalesces to the latest state.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::library;
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_artwork::media::image::material_you::{
    SchemeStyle, SeedCache, extract_source_rgb_from_rgb8, generate_palette,
};
use melodia_core::error::describe;
use melodia_core::themes::material3::MATERIAL3;
use melodia_core::themes::{Palette, SystemColorState};
use melodia_engine::player::engine::state::PlayerViewModelLight;

/// `repaint_tx` carries each snapshot to `ui::appearance::install`'s subscriber, `tasks/` naming no
/// `ui::*`.
pub fn spawn(
    spawner: &TaskSpawner,
    state: AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    view_model_rx: watch::Receiver<Option<PlayerViewModelLight>>,
    kick_rx: watch::Receiver<u64>,
    repaint_tx: watch::Sender<SystemColorState>,
    cover_thumbs: Arc<CoverThumbs>,
) {
    spawner.spawn_cancellable(move |shutdown| {
        run(state, os_state, view_model_rx, kick_rx, repaint_tx, cover_thumbs, shutdown)
    });
}

/// What was last applied, so a kick from an unrelated setting skips the quantize.
#[derive(Default)]
struct LastApplied {
    artwork: Option<PathBuf>,
    style: Option<SchemeStyle>,
    is_dark: Option<bool>,
    theme_id: Option<String>,
}

/// The three appearance fields `react` reads, refreshed only on a kick.
struct AppearanceSnapshot {
    theme_id: String,
    dynamic_color_style: String,
    theme_variant: String,
}

/// `None` on a failed read, the caller keeping whatever snapshot it had.
async fn read_appearance(state: &AppState) -> Option<AppearanceSnapshot> {
    let paths = Arc::clone(&state.paths);
    match tokio::task::spawn_blocking(move || library::settings::get_settings(&paths)).await {
        Ok(Ok(s)) => Some(AppearanceSnapshot {
            theme_id: s.theme_id,
            dynamic_color_style: s.dynamic_color_style,
            theme_variant: s.theme_variant,
        }),
        Ok(Err(e)) => {
            log::warn!("material_you: read settings: {}", describe(&e));
            None
        }
        Err(e) => {
            log::warn!("material_you: settings read join: {}", describe(&e));
            None
        }
    }
}

async fn run(
    state: AppState,
    os_state: Arc<RwLock<SystemColorState>>,
    mut view_model_rx: watch::Receiver<Option<PlayerViewModelLight>>,
    mut kick_rx: watch::Receiver<u64>,
    repaint_tx: watch::Sender<SystemColorState>,
    cover_thumbs: Arc<CoverThumbs>,
    shutdown: CancellationToken,
) {
    let mut seed_cache = SeedCache::new();
    let mut last = LastApplied::default();
    let mut appearance = read_appearance(&state).await;

    // Once at startup, the restored queue having seeded the view model before this spawns.
    react(
        &os_state,
        &mut seed_cache,
        &mut last,
        appearance.as_ref(),
        &view_model_rx,
        &repaint_tx,
        &cover_thumbs,
    )
    .await;

    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            res = view_model_rx.changed() => {
                if res.is_err() { break; }
            }
            res = kick_rx.changed() => {
                if res.is_err() { break; }
                // A failed read keeps the previous snapshot; the next kick retries.
                if let Some(fresh) = read_appearance(&state).await {
                    appearance = Some(fresh);
                }
            }
        }
        react(
            &os_state,
            &mut seed_cache,
            &mut last,
            appearance.as_ref(),
            &view_model_rx,
            &repaint_tx,
            &cover_thumbs,
        )
        .await;
    }
    log::debug!("material_you coordinator stopped");
}

async fn react(
    os_state: &Arc<RwLock<SystemColorState>>,
    seed_cache: &mut SeedCache,
    last: &mut LastApplied,
    appearance: Option<&AppearanceSnapshot>,
    view_model_rx: &watch::Receiver<Option<PlayerViewModelLight>>,
    repaint_tx: &watch::Sender<SystemColorState>,
    cover_thumbs: &Arc<CoverThumbs>,
) {
    // No snapshot yet: the startup read failed, and the next kick re-drives.
    let Some(appearance) = appearance else { return };

    let style = SchemeStyle::from_id(&appearance.dynamic_color_style);
    let theme_id = appearance.theme_id.clone();

    let os_theme = os_state.read().theme.clone();
    let is_dark = match appearance.theme_variant.as_str() {
        "dark" => true,
        "light" => false,
        _ => os_theme != "light", // "system" or anything unknown → OS says
    };

    let artwork = view_model_rx
        .borrow()
        .as_ref()
        .and_then(|vm| vm.current_track.as_ref())
        .and_then(|t| t.artwork_path.clone())
        .map(PathBuf::from);

    let should_clear = theme_id != MATERIAL3.id
        || style == SchemeStyle::None
        || artwork.as_ref().is_none_or(|p| !p.exists());

    if should_clear {
        let was_set = os_state.read().material_you.is_some();
        let was_inactive = last.artwork.is_none()
            && last.style == Some(SchemeStyle::None)
            && last.theme_id.as_deref() == Some(theme_id.as_str());
        if was_set || !was_inactive {
            os_state.write().material_you = None;
            *last = LastApplied {
                artwork: None,
                style: Some(SchemeStyle::None),
                is_dark: Some(is_dark),
                theme_id: Some(theme_id),
            };
            publish_repaint(repaint_tx, os_state);
        }
        return;
    }

    let Some(artwork) = artwork else { return };

    // `material_you` lives on `os_state`, so an OS theme flip while idle is caught here too.
    if last.artwork.as_ref() == Some(&artwork)
        && last.style == Some(style)
        && last.is_dark == Some(is_dark)
        && last.theme_id.as_deref() == Some(theme_id.as_str())
        && os_state.read().material_you.is_some()
    {
        return;
    }

    // Off the RGB8 thumbnail `CoverThumbs` already holds rather than a second decode, whose
    // transient glibc never fully returned. Inside the seed cache's closure, so a remembered seed
    // needs no buffer, and on the blocking pool, since a miss races
    // `ui::shell::bridge::warm_vm_cover` for the path and falls back to a full decode.
    let artwork_for_block = artwork.clone();
    let thumbs = cover_thumbs.clone();
    let mut tmp_cache = std::mem::take(seed_cache);
    let blocking_result = tokio::task::spawn_blocking(move || {
        let seed = tmp_cache.get_or_insert_with(&artwork_for_block, || {
            thumbs
                .get_or_load_rgb8(&artwork_for_block)
                .as_ref()
                .and_then(extract_source_rgb_from_rgb8)
        });
        let palette = seed.map(|s| generate_palette(s, is_dark, style));
        (tmp_cache, palette)
    })
    .await;

    let (returned_cache, palette) = match blocking_result {
        Ok(v) => v,
        Err(e) => {
            log::warn!("material_you: spawn_blocking join: {}", describe(&e));
            return;
        }
    };
    *seed_cache = returned_cache;

    let Some((palette, accent_hex)) = palette else {
        log::warn!("material_you: extraction failed for {}", artwork.display());
        os_state.write().material_you = None;
        publish_repaint(repaint_tx, os_state);
        return;
    };

    write_palette(os_state, palette, accent_hex);
    *last = LastApplied {
        artwork: Some(artwork),
        style: Some(style),
        is_dark: Some(is_dark),
        theme_id: Some(theme_id),
    };
    publish_repaint(repaint_tx, os_state);
}

fn write_palette(os_state: &Arc<RwLock<SystemColorState>>, palette: Palette, accent_hex: u32) {
    let mut guard = os_state.write();
    guard.material_you = Some((palette, accent_hex));
}

/// A `watch`, so a burst of generations during quick skips collapses to one repaint.
fn publish_repaint(
    repaint_tx: &watch::Sender<SystemColorState>,
    os_state: &Arc<RwLock<SystemColorState>>,
) {
    let snapshot = os_state.read().clone();
    if let Err(e) = repaint_tx.send(snapshot) {
        // Every receiver gone, which is the normal shutdown path.
        log::debug!("material_you: repaint subscriber dropped: {}", describe(&e));
    }
}

#[cfg(test)]
#[path = "tests/material_you_tests.rs"]
mod tests;
