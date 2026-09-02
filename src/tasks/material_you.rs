//! Material You coordinator task. Single tokio task that:
//!
//! 1. Subscribes to `view_model_rx` (current artwork path) and `ui::appearance`'s kick
//!    [`crate::state::Signal`] (theme / variant / style / system-theme changes).
//! 2. Keeps a cached `(theme_id, dynamic_color_style, theme_variant)`
//!    snapshot, re-read from `settings.json` only on kick wakes (every
//!    write to those fields kicks after persisting). On any wake it
//!    combines the snapshot with the current `os_state` and decides
//!    whether to generate or clear Material You — view-model wakes (the
//!    per-emit playback hot path) never touch the filesystem.
//! 3. CPU-bound extraction + palette generation runs on
//!    `tokio::task::spawn_blocking`. Source ARGB is cached in a bounded
//!    `SeedCache` so prev/next/prev round-trips don't re-quantize the
//!    same artwork.
//! 4. Writes `(Palette, accent_hex)` back into `os_state.material_you`
//!    and triggers a repaint via `slint::invoke_from_event_loop`.
//!
//! Both inputs are `tokio::sync::watch` receivers, so concurrent bursts
//! of clicks (e.g. user mashing the variant chip) auto-coalesce — the
//! coordinator only ever observes the latest state on its next wakeup.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::library;
use crate::media::cover_thumbs::CoverThumbs;
use crate::player::state::PlayerViewModelLight;
use crate::services::material_you::{
    SchemeStyle, SeedCache, extract_source_argb_from_rgb8, generate_palette,
};
use crate::state::AppState;
use crate::tasks::TaskSpawner;
use crate::themes::{Palette, SystemColorState};

/// Spawn the coordinator on the shared task lifecycle so the main shutdown
/// sequence waits for any in-flight palette generation to settle before the
/// runtime is torn down.
///
/// `repaint_tx` carries fresh `SystemColorState` snapshots to a UI-thread
/// subscriber installed by `ui::appearance::install`. The coordinator
/// never touches Slint state directly, keeping `tasks/` free of `ui::*`
/// imports.
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

/// Cached "what we last applied" tuple. The coordinator bails before
/// `spawn_blocking` if nothing relevant has changed — saves a quantize
/// round-trip when the kick was triggered by an unrelated setting.
#[derive(Default)]
struct LastApplied {
    artwork: Option<PathBuf>,
    style: Option<SchemeStyle>,
    is_dark: Option<bool>,
    theme_id: Option<String>,
}

/// The three appearance fields `react` consumes, snapshotted from
/// `settings.json` only on kick wakes. Every write to these fields is
/// followed by a kick (kick-after-persist convention), so view-model
/// wakes — the per-emit hot path: volume steps, seeks, play/pause —
/// reuse the cached copy instead of paying a disk read + JSON parse
/// per state emit.
struct AppearanceSnapshot {
    theme_id: String,
    dynamic_color_style: String,
    theme_variant: String,
}

/// Read the appearance fields off the blocking pool. Returns `None` on a
/// failed read so the caller can retain its previous snapshot (transient
/// failure) or bail until the next kick (no snapshot yet).
async fn read_appearance(state: &AppState) -> Option<AppearanceSnapshot> {
    let state = state.clone();
    match tokio::task::spawn_blocking(move || library::settings::get_settings(&state)).await {
        Ok(Ok(s)) => Some(AppearanceSnapshot {
            theme_id: s.theme_id,
            dynamic_color_style: s.dynamic_color_style,
            theme_variant: s.theme_variant,
        }),
        Ok(Err(e)) => {
            log::warn!("material_you: read settings: {e}");
            None
        }
        Err(e) => {
            log::warn!("material_you: settings read join: {e}");
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

    // Appearance snapshot, refreshed from disk only on kick wakes (every
    // theme / variant / style write kicks after persisting). View-model
    // wakes reuse it, so playback state emits never touch the filesystem.
    let mut appearance = read_appearance(&state).await;

    // Drive once at startup so an app launch with style != "none" picks
    // up the persisted artwork (queue restore seeds the view model
    // before this task spawns).
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
                // Keep the previous snapshot on a transient read failure —
                // the next kick re-attempts.
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

/// Single iteration: combine the cached appearance snapshot with the
/// latest view-model + `os_state`, decide whether to generate/clear/skip,
/// and publish the result through `repaint_tx` for the UI-thread
/// subscriber to apply.
async fn react(
    os_state: &Arc<RwLock<SystemColorState>>,
    seed_cache: &mut SeedCache,
    last: &mut LastApplied,
    appearance: Option<&AppearanceSnapshot>,
    view_model_rx: &watch::Receiver<Option<PlayerViewModelLight>>,
    repaint_tx: &watch::Sender<SystemColorState>,
    cover_thumbs: &Arc<CoverThumbs>,
) {
    // No snapshot yet (startup read failed) — nothing sensible to decide
    // on; the next kick re-reads and re-drives.
    let Some(appearance) = appearance else { return };

    let style = SchemeStyle::from_id(&appearance.dynamic_color_style);
    let theme_id = appearance.theme_id.clone();

    // Resolve `is_dark` from the user's variant + OS signal.
    let os_theme = os_state.read().theme.clone();
    let is_dark = match appearance.theme_variant.as_str() {
        "dark" => true,
        "light" => false,
        _ => os_theme != "light", // "system" or anything unknown → OS says
    };

    // Artwork path off the latest view-model snapshot.
    let artwork = view_model_rx
        .borrow()
        .as_ref()
        .and_then(|vm| vm.current_track.as_ref())
        .and_then(|t| t.artwork_path.clone())
        .map(PathBuf::from);

    // Decide.
    let should_clear = theme_id != "material3"
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

    // Skip work if nothing relevant changed since the last successful
    // generate. `material_you` lives on `os_state` so an OS theme tick
    // that flipped `is_dark` while we were idle is caught here.
    if last.artwork.as_ref() == Some(&artwork)
        && last.style == Some(style)
        && last.is_dark == Some(is_dark)
        && last.theme_id.as_deref() == Some(theme_id.as_str())
        && os_state.read().material_you.is_some()
    {
        return;
    }

    // Reuse the RGB8 thumbnail `CoverThumbs` already decoded for the
    // now-playing-bar / queue-sheet rows rather than opening the artwork a
    // second time — on large embedded covers the duplicate decode produced a
    // multi-MB transient that glibc didn't fully return to the OS, surfacing
    // as a ~30 MiB residual RSS bump whenever the current track was
    // DnD-imported. Inside the seed cache's closure and inside the blocking
    // task, both deliberately: a remembered seed needs no buffer at all, and
    // a miss races `ui::shell::bridge::warm_vm_cover` for the same path, so
    // the fallback is a full `decode_thumb_buffer` that has no business on a
    // runtime worker.
    let artwork_for_block = artwork.clone();
    let thumbs = cover_thumbs.clone();
    let mut tmp_cache = std::mem::take(seed_cache);
    let blocking_result = tokio::task::spawn_blocking(move || {
        let seed = tmp_cache.get_or_insert_with(&artwork_for_block, || {
            thumbs
                .get_or_load_rgb8(&artwork_for_block)
                .as_ref()
                .and_then(extract_source_argb_from_rgb8)
        });
        let palette = seed.map(|s| generate_palette(s, is_dark, style));
        (tmp_cache, palette)
    })
    .await;

    let (returned_cache, palette) = match blocking_result {
        Ok(v) => v,
        Err(e) => {
            log::warn!("material_you: spawn_blocking join: {e}");
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

/// Publish the latest `SystemColorState` snapshot for the UI-thread
/// subscriber to consume. `watch::send` keeps only the most recent value,
/// so a burst of generations during quick track skips collapses to one
/// repaint instead of N — exactly the coalescing behaviour the old
/// `invoke_from_event_loop` hop relied on implicitly.
fn publish_repaint(
    repaint_tx: &watch::Sender<SystemColorState>,
    os_state: &Arc<RwLock<SystemColorState>>,
) {
    let snapshot = os_state.read().clone();
    if let Err(e) = repaint_tx.send(snapshot) {
        // Only fails when every receiver has dropped, i.e. the UI is
        // tearing down. Logged at debug because it's the normal
        // shutdown path.
        log::debug!("material_you: repaint subscriber dropped: {e}");
    }
}
